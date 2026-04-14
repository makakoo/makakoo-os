//! `makakoo query` — FTS retrieval + LLM synthesis.

use makakoo_core::llm::ChatMessage;

use crate::context::CliContext;
use crate::output;

pub async fn run(
    ctx: &CliContext,
    question: &str,
    top_k: usize,
    model: &str,
) -> anyhow::Result<i32> {
    let store = ctx.store()?;
    let hits = store.search(question, top_k)?;
    if hits.is_empty() {
        output::print_warn("no Brain context matched — answering without retrieval");
    }
    let context_block = hits
        .iter()
        .map(|h| format!("- {}: {}", h.doc_id, h.content))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "You are Harvey, the user's autonomous cognitive extension. \
         Use ONLY this Brain context to answer the question. If the \
         context is insufficient, say so explicitly.\n\n\
         Question: {question}\n\n\
         Brain context:\n{context_block}"
    );
    let llm = ctx.llm();
    let answer = llm
        .chat(model, vec![ChatMessage::user(prompt)])
        .await?;
    println!("{}", answer.trim());
    Ok(0)
}
