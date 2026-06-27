use charles_mcp::session::{WsOpcode, chlsj};

#[test]
fn chlsj_text_websocket_frame() {
    let recv = include_str!("fixtures/ws_piesocket_recv.b64").trim();
    let json = format!(
        r#"[{{"webSocket":true,"scheme":"wss","host":"demo","method":"GET","path":"/",
            "response":{{"status":101,"body":{{"encoding":"base64","encoded":"{recv}"}}}}}}]"#
    );
    let txns = chlsj::parse(json.as_bytes()).unwrap();
    let ws = txns[0].websocket.as_ref().unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].opcode, WsOpcode::Text);
    let text = String::from_utf8_lossy(&ws[0].payload.bytes);
    assert!(text.contains("Unkown API Key"));
}
