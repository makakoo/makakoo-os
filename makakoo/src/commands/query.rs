//! `makakoo query` — FTS retrieval + memory-stack injection + LLM synthesis.

use std::sync::Arc;

use makakoo_core::llm::ChatMessage;
use makakoo_core::memory::{detect_intent, MemoryHarness};

use crate::context::CliContext;
use crate::output;

pub async fn run(
    ctx: &CliContext,
    question: &str,
    top_k: usize,
    model: &str,
    show_memory: bool,
) -> anyhow::Result<i32> {
    let store = ctx.store()?;
    let intent = detect_intent(question);

    let harness = MemoryHarness::new(ctx.home().clone(), Arc::clone(&store));
    let memory_block = harness.for_query(question, intent);

    if show_memory {
        println!("=== Memory ({:?}) ===", intent);
        if memory_block.is_empty() {
            println!("(empty)");
        } else {
            println!("{memory_block}");
        }
        println!("=== Answer ===");
    }

    let hits = store.search(question, top_k)?;
    if hits.is_empty() && memory_block.is_empty() {
        output::print_warn("no Brain context matched — answering without retrieval");
    }
    let context_block = hits
        .iter()
        .map(|h| format!("- {}: {}", h.doc_id, h.content))
        .collect::<Vec<_>>()
        .join("\n");

    let mut prompt = String::new();
    if !memory_block.is_empty() {
        prompt.push_str(&memory_block);
        prompt.push_str("\n\n");
    }
    prompt.push_str(
        "You are Harvey, the user's autonomous cognitive extension. \
         Use ONLY the Brain context to answer the question. If the \
         context is insufficient, say so explicitly.\n\n",
    );
    prompt.push_str(&format!("Question: {question}\n\n"));
    prompt.push_str(&format!("Brain context:\n{context_block}"));

    let llm = ctx.llm();
    let answer = llm
        .chat(model, vec![ChatMessage::user(prompt)])
        .await?;
    println!("{}", answer.trim());
    Ok(0)
}
