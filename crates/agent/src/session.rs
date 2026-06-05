use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    decide_call, filter_tools, mint, verify, AgentFrame, CallDecision, Claims, ErrorCode,
    TokenError, Tool, ViewerFrame,
};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::mcp::McpChild;
use crate::pidfile;

pub struct OpenArgs {
    pub server: String,
    pub server_args: Vec<String>,
    pub ttl: Duration,
    pub scope: Vec<String>,
    pub relay: String,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn random_id() -> String {
    let mut b = [0u8; 4];
    getrandom::getrandom(&mut b).expect("rng");
    hex::encode(b)
}

fn relay_host(relay: &str) -> String {
    relay.trim_start_matches("ws://").trim_start_matches("wss://").trim_end_matches('/').to_string()
}

fn err_frame(id: Option<u64>, code: ErrorCode, tool: Option<String>, msg: &str) -> AgentFrame {
    AgentFrame::Error { id, code, tool, message: Some(msg.to_string()) }
}

pub async fn open(args: OpenArgs) -> Result<()> {
    // 1. Spawn + handshake the child BEFORE dialing the relay (fail loud, leave nothing open).
    let mut child = McpChild::spawn(&args.server, &args.server_args).await?;
    let child_tools = child.tools.clone();

    // 2. Identity, secret, token.
    let tunnel_id = random_id();
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).map_err(|e| anyhow!("rng: {e}"))?;
    let exp = now_secs() + args.ttl.as_secs();
    let token = mint(&secret, &Claims { tunnel_id: tunnel_id.clone(), scope: args.scope.clone(), exp });

    // 3. Dial the relay (agent role).
    let host = relay_host(&args.relay);
    let agent_url = format!("{}/agent/{tunnel_id}", args.relay.trim_end_matches('/'));
    let (ws, _) = tokio_tungstenite::connect_async(&agent_url).await
        .map_err(|e| anyhow!("connecting to relay {agent_url}: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    // 4. Pidfile + banner.
    let link = format!("http://{host}/t/{tunnel_id}#{token}");
    pidfile::write(&tunnel_id, &link)?;
    print_banner(&args, &child_tools, &host, &tunnel_id, &token);

    // 5. Outbound frames -> relay sink.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<AgentFrame>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sink.send(Message::Text(serde_json::to_string(&frame).unwrap())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let shutdown = Arc::new(Notify::new());

    // 6. TTL timer: emit Expired, then trigger shutdown.
    {
        let out_tx = out_tx.clone();
        let shutdown = shutdown.clone();
        let secs = exp.saturating_sub(now_secs());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            let _ = out_tx.send(err_frame(None, ErrorCode::Expired, None, "ttl expired"));
            tokio::time::sleep(Duration::from_millis(150)).await; // let it flush
            eprintln!("\n  ttl expired — tunnel revoked. link is dead.");
            shutdown.notify_one();
        });
    }

    // 7. SIGINT/SIGTERM -> shutdown.
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).unwrap();
            let mut intr = signal(SignalKind::interrupt()).unwrap();
            tokio::select! {
                _ = term.recv() => {}
                _ = intr.recv() => {}
            }
            eprintln!("\n  revoked. link is dead.");
            shutdown.notify_one();
        });
    }

    // 8. Event loop. Verified claims cached after a good Hello; exp re-checked per call.
    let mut verified: Option<Claims> = None;
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            msg = stream.next() => {
                let txt = match msg {
                    Some(Ok(Message::Text(txt))) => txt,
                    Some(Ok(_)) => continue,       // ignore binary/ping/pong/close-as-message
                    None | Some(Err(_)) => break,  // relay/viewer gone → tear down
                };
                let frame: ViewerFrame = match serde_json::from_str(&txt) {
                    Ok(f) => f,
                    Err(_) => { let _ = out_tx.send(err_frame(None, ErrorCode::BadRequest, None, "unparseable frame")); continue; }
                };
                if handle_frame(frame, &secret, exp, &child_tools, &mut verified, &mut child, &out_tx).await.is_break() {
                    break; // invalid/expired Hello closes the session (spec §7.3)
                }
            }
        }
    }

    // Teardown (one path for TTL, signal, and relay-close).
    child.kill().await;
    pidfile::remove(&tunnel_id);
    writer.abort();
    Ok(())
}

/// Handle one viewer frame. Returns `Break` when the session must close
/// (an invalid or expired `Hello`, per spec §7.3); `Continue` otherwise.
async fn handle_frame(
    frame: ViewerFrame,
    secret: &[u8],
    exp: u64,
    child_tools: &[Tool],
    verified: &mut Option<Claims>,
    child: &mut McpChild,
    out_tx: &mpsc::UnboundedSender<AgentFrame>,
) -> ControlFlow<()> {
    match frame {
        ViewerFrame::Hello { token } => match verify(secret, &token, now_secs()) {
            Ok(c) => {
                let expires_in_ms = exp.saturating_sub(now_secs()) * 1000;
                let _ = out_tx.send(AgentFrame::Ready { scope: c.scope.clone(), expires_in_ms });
                let _ = out_tx.send(AgentFrame::Tools { tools: filter_tools(child_tools, &c.scope) });
                *verified = Some(c);
            }
            Err(TokenError::Expired) => {
                let _ = out_tx.send(err_frame(None, ErrorCode::Expired, None, "token expired"));
                return ControlFlow::Break(());
            }
            Err(_) => {
                let _ = out_tx.send(err_frame(None, ErrorCode::Unauthorized, None, "invalid token"));
                return ControlFlow::Break(());
            }
        },
        ViewerFrame::List => match verified {
            Some(c) => { let _ = out_tx.send(AgentFrame::Tools { tools: filter_tools(child_tools, &c.scope) }); }
            None => { let _ = out_tx.send(err_frame(None, ErrorCode::Unauthorized, None, "say hello first")); }
        },
        ViewerFrame::Call { id, tool, args } => {
            let Some(c) = verified.clone() else {
                let _ = out_tx.send(err_frame(Some(id), ErrorCode::Unauthorized, None, "say hello first"));
                return ControlFlow::Continue(());
            };
            match decide_call(&c, now_secs(), &tool) {
                CallDecision::Expired => { let _ = out_tx.send(err_frame(Some(id), ErrorCode::Expired, None, "ttl expired")); }
                CallDecision::OutOfScope => { let _ = out_tx.send(err_frame(Some(id), ErrorCode::OutOfScope, Some(tool), "tool not in scope")); }
                CallDecision::Forward => match child.call_tool(&tool, args).await {
                    Ok(result) => {
                        let content = result.get("content").cloned().unwrap_or(result);
                        let _ = out_tx.send(AgentFrame::Result { id, content });
                    }
                    Err(e) => { let _ = out_tx.send(err_frame(Some(id), ErrorCode::ToolError, Some(tool), &e.to_string())); }
                },
            }
        }
    }
    ControlFlow::Continue(())
}

pub fn close(tunnel_id: Option<&str>) -> Result<()> {
    let (pid, id) = pidfile::read(tunnel_id)?;
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid), Signal::SIGTERM).map_err(|e| anyhow!("signalling pid {pid}: {e}"))?;
    println!("  revoked tunnel {id} (pid {pid}). link is dead.");
    Ok(())
}

fn print_banner(args: &OpenArgs, child_tools: &[Tool], host: &str, id: &str, token: &str) {
    let in_scope = child_tools.iter().filter(|t| args.scope.iter().any(|s| s == &t.name)).count();
    let total = child_tools.len();
    let scope_str = if args.scope.is_empty() { "(none — deny all)".into() } else { args.scope.join(",") };
    println!();
    println!("  tunnel.solutions");
    println!("  ─────────────────");
    println!("  spawn      {} … ok", args.server);
    println!("  handshake  ✓   MCP initialize — {total} tools advertised");
    println!("  scope      {scope_str}    ({in_scope} of {total} in scope)");
    println!("  ttl        {}", humantime::format_duration(args.ttl));
    println!("  token      ✓   minted");
    println!("  relay      {}  connected", args.relay);
    println!("  link  →    http://{host}/t/{id}#{token}");
    println!();
    println!("  serving — ctrl-c or `tunnel close` to revoke");
    println!();
}
