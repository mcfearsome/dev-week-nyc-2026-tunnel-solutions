//! The `describe` contract shared by the `tnls` host (deserializes) and every plugin
//! (serializes). Keep this crate tiny — it is the only thing both sides depend on.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub name: String,
    pub about: String,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub about: String,
    /// `Some` => tunneled (the agent path); `None`/absent => plain passthrough.
    #[serde(default)]
    pub tunnel: Option<Tunnel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    pub scope: Vec<String>,
    pub default_ttl: String,
}

impl Manifest {
    pub fn new(name: &str, about: &str, commands: Vec<Command>) -> Self {
        Self { schema: 1, name: name.into(), about: about.into(), commands }
    }
    pub fn command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.name == name)
    }
    /// The single JSON line a plugin's `describe` prints.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("manifest serializes")
    }
}

impl Command {
    pub fn tunneled(name: &str, about: &str, scope: &[&str], default_ttl: &str) -> Self {
        Self {
            name: name.into(),
            about: about.into(),
            tunnel: Some(Tunnel { scope: scope.iter().map(|s| s.to_string()).collect(), default_ttl: default_ttl.into() }),
        }
    }
    pub fn plain(name: &str, about: &str) -> Self {
        Self { name: name.into(), about: about.into(), tunnel: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_with_tunnel_and_plain() {
        let m = Manifest::new(
            "rendezvous",
            "Capability-scoped file sending over BitTorrent.",
            vec![
                Command::tunneled("share", "Seed + serve a file.", &["list_shares", "request_file"], "30m"),
                Command::plain("get", "Fetch a shared file."),
            ],
        );
        let json = m.to_json();
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, 1);
        assert_eq!(back.command("share").unwrap().tunnel.as_ref().unwrap().scope, vec!["list_shares", "request_file"]);
        assert!(back.command("get").unwrap().tunnel.is_none(), "plain command has no tunnel");
        assert!(json.contains(r#""tunnel":null"#), "plain command serializes tunnel:null");
    }
}
