# Kompatibilität für pip-open_clip: neuere HF-Tokenizer (z. B. GemmaTokenizer)
# haben kein batch_encode_plus. Wir wrappen die Instanz und nutzen tokenizer(...).

from typing import List, Union, Optional
import torch


class _HFTokenizerWrapper:
    """Wrappt open_clip HFTokenizer und nutzt in __call__ die moderne HF-API."""

    def __init__(self, inner):
        self._inner = inner

    def __call__(self, texts: Union[str, List[str]], context_length: Optional[int] = None) -> torch.Tensor:
        if isinstance(texts, str):
            texts = [texts]
        ctx = context_length or self._inner.context_length
        assert ctx, "Please set a valid context length in class init or call."
        texts = [self._inner.clean_fn(t) for t in texts]

        if getattr(self._inner, "tokenizer_mode", "") == "clips":
            return self._clips_tokenize(texts, ctx)

        encoded = self._inner.tokenizer(
            texts,
            return_tensors="pt",
            max_length=ctx,
            padding="max_length",
            truncation=True,
        )
        input_ids = encoded.input_ids
        if getattr(self._inner, "strip_sep_token", False) and getattr(
            self._inner.tokenizer, "sep_token_id", None
        ) is not None:
            input_ids = torch.where(
                input_ids == self._inner.tokenizer.sep_token_id,
                torch.zeros_like(input_ids),
                input_ids,
            )
        return input_ids

    def _clips_tokenize(self, texts: List[str], context_length: int) -> torch.Tensor:
        enc = self._inner.tokenizer(
            texts,
            add_special_tokens=False,
            padding=False,
            truncation=False,
            return_tensors=None,
        )
        encoded = []
        for tokens in enc["input_ids"]:
            tokens = tokens[: context_length - 3]
            tokens = [
                self._inner.tokenizer.bos_token_id,
                *tokens,
                self._inner.tokenizer.eos_token_id,
            ]
            encoded.append(tokens)
        result = torch.zeros(len(encoded), context_length, dtype=torch.long)
        for i, tokens in enumerate(encoded):
            padded = self._inner._pad_and_add_class_token(
                tokens,
                max_length=context_length,
                pad_token_id=self._inner.tokenizer.pad_token_id,
                cls_token_id=self._inner.tokenizer.cls_token_id,
            )
            result[i, : len(padded)] = torch.tensor(padded)
        return result

    def __getattr__(self, name):
        return getattr(self._inner, name)


def wrap_tokenizer(tok):
    """Wrappt Tokenizer, wenn er kein batch_encode_plus hat (z. B. open_clip HFTokenizer mit Gemma)."""
    if tok is None:
        return None
    try:
        from open_clip.tokenizer import HFTokenizer
        if isinstance(tok, HFTokenizer) and not hasattr(tok.tokenizer, "batch_encode_plus"):
            return _HFTokenizerWrapper(tok)
    except Exception:
        pass
    return tok
