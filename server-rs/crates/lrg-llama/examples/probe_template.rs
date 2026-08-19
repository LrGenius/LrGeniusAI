//! Prints what a GGUF reports as its chat template and whether llama.cpp will
//! apply it. Diagnostic only: `llama_chat_apply_template` answers an
//! unrecognised template with a bare `-1`, which is indistinguishable from any
//! other failure, so this narrows it down.
//!
//! ```sh
//! cargo run -p lrg-llama --example probe_template -- /path/to/model.gguf
//! ```

use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_template <gguf>");
    let backend = LlamaBackend::init().expect("backend");
    let model = LlamaModel::load_from_file(&backend, &path, &LlamaModelParams::default())
        .expect("load model");

    let user = LlamaChatMessage::new("user".into(), "Hello.".into()).unwrap();
    let system = LlamaChatMessage::new("system".into(), "Be brief.".into()).unwrap();

    for key in [
        "general.architecture",
        "general.name",
        "general.basename",
        "tokenizer.ggml.model",
        "tokenizer.chat_template",
    ] {
        match model.meta_val_str(key) {
            Ok(v) => println!("META {key} = {:?}", v.chars().take(70).collect::<String>()),
            Err(e) => println!("META {key} = <{e}>"),
        }
    }

    // Does the vocabulary actually know Gemma's turn tokens? If it does, the
    // built-in gemma template emits tokens the model was trained on.
    for probe in ["<start_of_turn>", "<|im_start|>", "<end_of_turn>"] {
        let toks = model.str_to_token(probe, llama_cpp_2::model::AddBos::Never);
        println!("VOCAB {probe} -> {:?}", toks.map(|t| t.len()));
    }

    // The model's own template, then llama.cpp's built-ins by name. Anything
    // that renders both shapes is a usable stand-in.
    let mut candidates: Vec<(String, llama_cpp_2::model::LlamaChatTemplate)> = Vec::new();
    if let Ok(tmpl) = model.chat_template(None) {
        candidates.push(("<model>".to_string(), tmpl));
    }
    for name in ["gemma", "gemma2", "gemma3", "chatml", "llama3", "vicuna"] {
        if let Ok(tmpl) = llama_cpp_2::model::LlamaChatTemplate::new(name) {
            candidates.push((name.to_string(), tmpl));
        }
    }

    for (label, tmpl) in &candidates {
        for (shape, msgs) in [
            ("user", vec![user.clone()]),
            ("system+user", vec![system.clone(), user.clone()]),
        ] {
            match model.apply_chat_template(tmpl, &msgs, true) {
                Ok(rendered) => println!("[{label} / {shape}] OK: {:?}", rendered),
                Err(e) => println!("[{label} / {shape}] ERR {e}"),
            }
        }
    }
}
