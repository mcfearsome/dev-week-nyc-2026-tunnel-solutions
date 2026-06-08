use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio_tungstenite::tungstenite::Message;
use tunnel_locker_core::{AgentFrame, ViewerFrame};

#[derive(Debug)]
pub struct RetrievedFile {
    pub magnet: String,
    pub peers: Vec<SocketAddr>,
    pub name: Option<String>,
}

/// Parse the `request_file` content text: JSON `{magnet, peers}` or a bare `magnet:` string.
pub fn parse_request_file_content(text: &str) -> (String, Vec<SocketAddr>) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(magnet) = v.get("magnet").and_then(|m| m.as_str()) {
            let peers = v
                .get("peers")
                .and_then(|p| p.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .filter_map(|s| s.parse().ok())
                        .collect()
                })
                .unwrap_or_default();
            return (magnet.to_string(), peers);
        }
    }
    (text.to_string(), Vec::new()) // bare magnet
}

fn name_from_magnet(magnet: &str) -> Option<String> {
    magnet
        .split("&dn=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .map(|s| {
            urlencoding::decode(s)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| s.to_string())
        })
}

/// (ws_viewer_url, tunnel_id, token) from an `https://host/t/<id>#<token>` link.
pub fn parse_link(link: &str) -> Result<(String, String, String)> {
    let (before_hash, token) = link.split_once('#').context("link missing #token")?;
    let scheme_ws = if before_hash.starts_with("https://") {
        "wss"
    } else {
        "ws"
    };
    let rest = before_hash
        .split("://")
        .nth(1)
        .context("link missing scheme")?;
    let (host, idpart) = rest.split_once("/t/").context("link missing /t/")?;
    let id = idpart.trim_end_matches('/');
    if id.is_empty() || token.is_empty() {
        bail!("link missing id or token");
    }
    Ok((
        format!("{scheme_ws}://{host}/viewer/{id}"),
        id.to_string(),
        token.to_string(),
    ))
}

/// Connect as a headless viewer and call `request_file` to obtain the magnet + peers.
pub async fn retrieve_magnet(link: &str) -> Result<RetrievedFile> {
    let (ws_url, _id, token) = parse_link(link)?;
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .with_context(|| format!("connecting to {ws_url}"))?;

    ws.send(Message::Text(serde_json::to_string(&ViewerFrame::Hello {
        token,
    })?))
    .await?;

    // read until both Ready and Tools have arrived (two separate frames)
    let (mut ready, mut tools) = (false, false);
    while !(ready && tools) {
        match next_frame(&mut ws).await? {
            AgentFrame::Ready { .. } => ready = true,
            AgentFrame::Tools { .. } => tools = true,
            AgentFrame::Error { code, message, .. } => {
                bail!("link dead: {code:?} {}", message.unwrap_or_default())
            }
            _ => {}
        }
    }

    ws.send(Message::Text(serde_json::to_string(&ViewerFrame::Call {
        id: 1,
        tool: "request_file".into(),
        args: serde_json::json!({}),
    })?))
    .await?;

    loop {
        match next_frame(&mut ws).await? {
            AgentFrame::Result { content, .. } => {
                let text = content
                    .get(0)
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.as_str())
                    .context("request_file result had no text")?
                    .to_string();
                let (magnet, peers) = parse_request_file_content(&text);
                let name = name_from_magnet(&magnet);
                return Ok(RetrievedFile {
                    magnet,
                    peers,
                    name,
                });
            }
            AgentFrame::Error { code, message, .. } => {
                bail!("refused: {code:?} {}", message.unwrap_or_default())
            }
            _ => {}
        }
    }
}

async fn next_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<AgentFrame> {
    loop {
        let msg = ws.next().await.ok_or_else(|| anyhow!("link closed"))??;
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).context("decoding agent frame");
        }
    }
}

pub async fn run_get(link: &str, out_dir: &std::path::Path) -> Result<()> {
    let rf = retrieve_magnet(link).await?;
    println!("magnet acquired — downloading…");
    std::fs::create_dir_all(out_dir)?;
    let dl = crate::bittorrent::fetch(
        &rf.magnet,
        out_dir,
        crate::bittorrent::NetOpts {
            disable_dht: false,
            listen_port: None,
            enable_upnp: true,
            initial_peers: rf.peers,
        },
    )
    .await?;
    loop {
        let p = dl.progress();
        let pct = if p.total > 0 {
            p.downloaded * 100 / p.total
        } else {
            0
        };
        println!("  {pct:>3}%  {}/{} bytes", p.downloaded, p.total);
        if p.finished {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    println!("done → {:?}", dl.output_path());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_https_link() {
        let (ws, id, tok) = parse_link("https://tnls.to/t/3f9a8c#eyJhbGc.sig").unwrap();
        assert_eq!(ws, "wss://tnls.to/viewer/3f9a8c");
        assert_eq!(id, "3f9a8c");
        assert_eq!(tok, "eyJhbGc.sig");
    }
    #[test]
    fn parses_local_http_link() {
        let (ws, _, _) = parse_link("http://127.0.0.1:8787/t/abcd#tok").unwrap();
        assert_eq!(ws, "ws://127.0.0.1:8787/viewer/abcd");
    }
    #[test]
    fn rejects_garbage() {
        assert!(parse_link("nope").is_err());
    }
    #[test]
    fn parses_json_magnet_with_peers() {
        let (m, peers) = parse_request_file_content(
            r#"{"magnet":"magnet:?xt=urn:btih:zz","peers":["127.0.0.1:6881","bad"]}"#,
        );
        assert_eq!(m, "magnet:?xt=urn:btih:zz");
        assert_eq!(
            peers,
            vec!["127.0.0.1:6881".parse::<std::net::SocketAddr>().unwrap()]
        ); // "bad" skipped
    }
    #[test]
    fn falls_back_to_bare_magnet() {
        let (m, peers) = parse_request_file_content("magnet:?xt=urn:btih:zz&dn=x");
        assert_eq!(m, "magnet:?xt=urn:btih:zz&dn=x");
        assert!(peers.is_empty());
    }
}
