//! Retrieval harness for devstate usage, not project memory or source search.
//! Curated versioned topics are bundled with the binary; no model/API/index DB.

#[derive(Debug, serde::Serialize)]
pub struct GuideHit {
    pub id: &'static str,
    pub title: &'static str,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct GuideResult {
    pub query: String,
    pub topics: Vec<GuideHit>,
    pub more_matches: bool,
    pub available_topics: Vec<&'static str>,
}

struct Topic {
    id: &'static str,
    title: &'static str,
    keywords: &'static [&'static str],
    content: &'static str,
}

const TOPICS: &[Topic] = &[
    Topic { id: "start", title: "Start a project", keywords: &["init", "start", "setup", "เริ่ม", "ติดตั้ง"], content: include_str!("../docs/guide/start.md") },
    Topic { id: "resume", title: "Continue focused work", keywords: &["resume", "continue", "focus", "ทำต่อ", "ค้าง", "checkpoint"], content: include_str!("../docs/guide/resume.md") },
    Topic { id: "tasks", title: "Find and inspect tasks", keywords: &["task", "search", "bug", "notes", "ค้น", "บัค", "โน้ต", "รายละเอียด"], content: include_str!("../docs/guide/tasks.md") },
    Topic { id: "overview", title: "Work across projects", keywords: &["overview", "registry", "register", "global", "หลายโปรเจ", "ทุกโปรเจ"], content: include_str!("../docs/guide/overview.md") },
    Topic { id: "fingerprint", title: "Interpret verification freshness", keywords: &["fingerprint", "verify", "verification", "stale", "เทสต์", "ทดสอบ", "ผลตรวจ"], content: include_str!("../docs/guide/fingerprint.md") },
    Topic { id: "tokens", title: "Keep agent context small", keywords: &["token", "context", "budget", "ประหยัด", "ลด", "usage", "model"], content: include_str!("../docs/guide/tokens.md") },
    Topic { id: "limits", title: "Current implementation limits", keywords: &["limits", "create", "close", "mutation", "บันทึก", "สร้างงาน", "ปิดงาน"], content: include_str!("../docs/guide/limits.md") },
];

/// Retrieve at most five topics and 1800 content characters in total. This is a
/// character budget, not a tokenizer-specific guarantee. Unknown queries do not
/// fabricate guidance: return the small topic catalog for a more precise lookup.
pub fn retrieve(query: &str, limit: u32) -> crate::store::Result<GuideResult> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 200 || !(1..=5).contains(&limit) {
        return Err("guide requires 1..=200 query characters and a limit of 1..=5".into());
    }
    let folded = query.to_lowercase();
    let words: Vec<_> = folded.split_whitespace().collect();
    let mut ranked: Vec<_> = TOPICS.iter().filter_map(|topic| {
        let score = if folded == topic.id { 100 } else {
            topic.keywords.iter().filter(|keyword| folded.contains(**keyword)).count() * 5
                + words.iter().filter(|word| topic.content.to_lowercase().contains(**word)).count()
        };
        (score > 0).then_some((score, topic))
    }).collect();
    ranked.sort_by(|(a_score, a), (b_score, b)| b_score.cmp(a_score).then(a.id.cmp(b.id)));
    let mut remaining = 1800;
    let mut topics = Vec::new();
    for (_, topic) in ranked.iter().take(limit as usize) {
        if remaining == 0 { break; }
        let content: String = topic.content.chars().take(remaining).collect();
        remaining -= content.chars().count();
        topics.push(GuideHit { id: topic.id, title: topic.title,
            truncated: content.chars().count() < topic.content.chars().count(), content });
    }
    let more_matches = ranked.len() > topics.len();
    let available_topics = if topics.is_empty() { TOPICS.iter().map(|topic| topic.id).collect() } else { vec![] };
    Ok(GuideResult { query: query.into(), topics, more_matches, available_topics })
}
