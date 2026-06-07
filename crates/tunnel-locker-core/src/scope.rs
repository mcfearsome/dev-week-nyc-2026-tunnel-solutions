use crate::frame::Tool;
use crate::token::Claims;

impl Claims {
    pub fn allows(&self, tool: &str) -> bool {
        self.scope.iter().any(|s| s == tool)
    }
}

/// Keep only tools whose name is in scope. Produces the filtered `tools/list`.
pub fn filter_tools(all: &[Tool], scope: &[String]) -> Vec<Tool> {
    all.iter().filter(|t| scope.iter().any(|s| s == &t.name)).cloned().collect()
}

/// The per-call enforcement decision. Order matters: TTL is checked before scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallDecision {
    Forward,
    Expired,
    OutOfScope,
}

pub fn decide_call(claims: &Claims, now: u64, tool: &str) -> CallDecision {
    if now >= claims.exp {
        return CallDecision::Expired;
    }
    if !claims.allows(tool) {
        return CallDecision::OutOfScope;
    }
    CallDecision::Forward
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claims() -> Claims {
        Claims { tunnel_id: "t".into(), scope: vec!["read".into()], exp: 1000 }
    }

    fn tool(name: &str) -> Tool {
        Tool { name: name.into(), description: None, input_schema: json!({}) }
    }

    #[test]
    fn filter_keeps_only_in_scope() {
        let all = vec![tool("read"), tool("shell")];
        let kept = filter_tools(&all, &["read".into()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "read");
    }

    #[test]
    fn empty_scope_filters_everything() {
        let all = vec![tool("read"), tool("shell")];
        assert!(filter_tools(&all, &[]).is_empty());
    }

    #[test]
    fn in_scope_and_valid_forwards() {
        assert_eq!(decide_call(&claims(), 999, "read"), CallDecision::Forward);
    }

    #[test]
    fn out_of_scope_refused() {
        assert_eq!(decide_call(&claims(), 999, "shell"), CallDecision::OutOfScope);
    }

    #[test]
    fn expired_beats_out_of_scope() {
        // Both conditions true; expiry must win (revocation is absolute).
        assert_eq!(decide_call(&claims(), 1000, "shell"), CallDecision::Expired);
    }
}
