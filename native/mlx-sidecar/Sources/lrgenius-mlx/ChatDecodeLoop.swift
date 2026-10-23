// Adapted from mlx-swift-lm's `GuidedGenerationLoop` (MIT License,
// Copyright © 2026 Apple Inc.), at the revision pinned in Package.swift.
//
// Why a copy: the library's loop makes its own KV cache, and a chat turn has
// to start from the cache the previous turn left (see `Engine.chat`). So this
// one takes the caller's cache, says when the prompt is in it, and reports
// every token it fed through the model — which is what the cache then holds.
//
// One fix over the library: after a sampled token the grammar can force more
// (fast-forward), and the library fed only the forced tokens through the
// model, never the sampled one before them. The model then answered without
// ever seeing tokens it had chosen itself. Here the sampled token goes in
// first.
//
// Left out: KV quantization and the diagnostic mask snapshots, which nothing
// here uses. Indexing still runs the library's loop.

import MLX
import MLXGuidedGeneration
import MLXLMCommon

enum ChatDecodeLoop {

    struct Outcome {
        /// Tokens produced, sampled and forced alike.
        let produced: Int
        /// Every token fed through the model after the prompt, in order. The
        /// cache holds the prompt followed by exactly these.
        let fed: [Int]
        /// Tokens the grammar matcher accepted — each sampled token and every
        /// forced one, counted or not. Rolling back this many leaves the
        /// constraint as it was compiled.
        let accepted: Int
    }

    /// Prefill `input` on top of whatever `cache` already holds, call
    /// `onPrefilled` once it is in, then decode against `constraint`,
    /// handing text to `emit` (return `false` to stop).
    ///
    /// Budget handling is the library's: `completionReserve` tokens before
    /// `maxTokens`, `closingBias` nudges towards closing the JSON value, and
    /// `whitespaceBias` holds down runs of whitespace.
    static func run(
        input: LMInput,
        cache: [KVCache],
        context: ModelContext,
        constraint: GrammarConstraint,
        maxTokens: Int,
        vocabSize: Int,
        completionReserve: Int,
        closingBias: MLXArray?,
        whitespaceBias: MLXArray?,
        whitespaceTokenIDs: Set<Int>,
        onPrefilled: () -> Void,
        emit: (String) -> Bool
    ) throws -> Outcome {
        let model = context.model
        let diagnosticSink = GuidedGenerationDiagnosticSink.current
        let stopTokenIDs = stopTokens(
            tokenizer: context.tokenizer, configuration: context.configuration)

        var modelState: LMOutput.State?
        var logits: MLXArray
        switch try model.prepare(
            input, cache: cache, state: nil,
            prefill: .init(stepSize: PrefillParameters.defaultStepSize))
        {
        case .tokens(let tokens):
            let result = model(tokens[text: .newAxis], cache: cache, state: nil)
            modelState = result.state
            logits = result.logits
        case .logits(let result):
            modelState = result.state
            logits = result.logits
        }
        eval(logits)
        onPrefilled()

        var fed: [Int] = []
        var accepted = 0
        // Feeds one token at a time: several at once with a populated cache
        // trip an MLX attention-mask bug on models with global attention
        // layers (Gemma 3), says the library.
        func feed(_ token: Int) {
            let step = LMInput.Text(tokens: MLXArray([Int32(token)]))
            let result = model(
                step[text: .newAxis], cache: cache.isEmpty ? nil : cache, state: modelState)
            modelState = result.state
            logits = result.logits
            fed.append(token)
        }

        var detokenizer = NaiveStreamingDetokenizer(tokenizer: context.tokenizer)
        var tokenCount = 0
        var grammarStopped = false
        var whitespaceTracker = WhitespaceRunTracker(whitespaceTokenIDs: whitespaceTokenIDs)
        let logitDim = logits.shape[logits.ndim - 1]
        var mask = try constraint.computeMask()
        var maskArray = additiveMask(for: mask, vocabSize: vocabSize, logitDim: logitDim)

        while tokenCount < maxTokens {
            try Task.checkCancellation()
            if mask.isTerminated {
                grammarStopped = true
                break
            }

            // The library's zones: none until `completionReserve` tokens are
            // left, then the closing bias. Only where the grammar actually
            // masks something; an unconditional splice needs no nudge.
            var activeBias: MLXArray?
            if mask.needsApply {
                if let closingBias, tokenCount >= maxTokens - completionReserve {
                    activeBias = closingBias
                }
                if let whitespaceBias, whitespaceTracker.isActive {
                    activeBias = activeBias.map { $0 + whitespaceBias } ?? whitespaceBias
                }
            }
            let token = Int(sample(logits: logits, maskArray: maskArray, bias: activeBias))
            if whitespaceBias != nil {
                _ = whitespaceTracker.record(tokenID: token)
            }
            // With no mask in force the grammar splices regardless of what was
            // sampled, so a stop token then means nothing.
            if mask.needsApply
                && (token == context.tokenizer.unknownTokenId || stopTokenIDs.contains(token))
            {
                grammarStopped = true
                break
            }

            let commit = try constraint.commitToken(Int32(token))
            accepted += 1 + commit.tokens.count
            diagnosticSink?.recordSampledToken(token)
            detokenizer.append(token: token)
            if let text = detokenizer.next(), !emit(text) { break }
            tokenCount += 1
            if commit.isTerminated {
                grammarStopped = true
                break
            }

            // Tokens the grammar forces next. The sampled one is not among
            // them and goes through the model first.
            var stop = false
            var forced: [Int] = []
            for ffToken in commit.tokens.map(Int.init) {
                if tokenCount >= maxTokens {
                    stop = true
                    break
                }
                diagnosticSink?.recordFastForwardToken(ffToken)
                forced.append(ffToken)
                tokenCount += 1
                detokenizer.append(token: ffToken)
                if let text = detokenizer.next(), !emit(text) {
                    stop = true
                    break
                }
            }
            if stop { break }

            feed(token)
            forced.forEach(feed)
            asyncEval(logits)
            // Overlap: the next mask is built on the CPU while the GPU runs.
            mask = try constraint.computeMask()
            maskArray = additiveMask(for: mask, vocabSize: vocabSize, logitDim: logitDim)
            eval(logits)
        }

        diagnosticSink?.recordTermination(
            grammarTerminated: grammarStopped, generatedTokenCount: tokenCount)
        if !grammarStopped && tokenCount >= maxTokens {
            throw GuidedGenerationError.incompleteOutput
        }
        return Outcome(produced: tokenCount, fed: fed, accepted: accepted)
    }

    /// The tokens that end an answer: the configuration's EOS ids (Gemma
    /// lists `<end_of_turn>` there), the tokenizer's own, and the
    /// configuration's extra EOS strings.
    private static func stopTokens(
        tokenizer: any Tokenizer, configuration: ModelConfiguration
    ) -> Set<Int> {
        var ids = Set(configuration.eosTokenIds)
        if let eos = tokenizer.eosTokenId {
            ids.insert(eos)
        }
        for token in configuration.extraEOSTokens {
            if let id = tokenizer.convertTokenToId(token) {
                ids.insert(id)
            }
        }
        return ids
    }

    /// Greedy pick from the last position's logits, after the grammar mask
    /// and the bias. The bias is sized to the tokenizer, which can be shorter
    /// or longer than the model's logits.
    private static func sample(logits rawLogits: MLXArray, maskArray: MLXArray?, bias: MLXArray?)
        -> UInt32
    {
        var logits = rawLogits[0..., -1, 0...]
        if let maskArray {
            logits = logits + maskArray
        }
        if let bias {
            let logitDim = logits.shape[logits.ndim - 1]
            let biasDim = bias.shape[0]
            if biasDim < logitDim {
                logits = logits + concatenated([bias, MLXArray.zeros([logitDim - biasDim])])
            } else if biasDim > logitDim {
                logits = logits + bias[0 ..< logitDim]
            } else {
                logits = logits + bias
            }
        }
        return argMax(logits, axis: -1).item(UInt32.self)
    }

    /// The grammar mask as an additive array (0 allowed, -inf not), or `nil`
    /// when the grammar masks nothing.
    private static func additiveMask(for mask: MaskResult, vocabSize: Int, logitDim: Int)
        -> MLXArray?
    {
        guard mask.needsApply else { return nil }
        return mask.mask.withUnsafeBufferPointer { buffer in
            let words = UnsafeRawPointer(buffer.baseAddress!).assumingMemoryBound(
                to: UInt32.self)
            var floats = [Float](repeating: -Float.infinity, count: logitDim)
            for i in 0 ..< min(vocabSize, logitDim)
            where (words[i / 32] >> (UInt32(i) % 32)) & 1 == 1 {
                floats[i] = 0
            }
            return MLXArray(floats)
        }
    }
}
