use wire_protocol::{
    parse_subscription_uri, vless_build_request, vless_parse_request, vmess_aead_decode_chunk,
    vmess_aead_encode_chunk, vmess_client_header, vmess_server_open_header, VlessAddress,
    VlessParseResult, WireMode,
};
use uuid::Uuid;

#[test]
fn parse_vless_vmess_trojan() {
    let v = parse_subscription_uri("vless://f47e3452-6ea8-4f3a-9c2b-1a0b9c8d7e6f@example.com:443?type=tcp")
        .unwrap();
    assert_eq!(v.mode, WireMode::Vless);
    assert_eq!(v.remote_host, "example.com");
    assert_eq!(v.remote_port, 443);
    assert!(v.params.uuid.as_ref().unwrap().contains("f47e3452"));

    let t = parse_subscription_uri("trojan://secret@t.example.org:8443").unwrap();
    assert_eq!(t.mode, WireMode::Trojan);
    assert_eq!(t.params.password.as_deref(), Some("secret"));

    let vmess_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        br#"{"v":"2","ps":"x","add":"v.example.net","port":"443","id":"f47e3452-6ea8-4f3a-9c2b-1a0b9c8d7e6f","aid":"0","net":"tcp","type":"none","host":"","path":"","tls":""}"#,
    );
    let vm = parse_subscription_uri(&format!("vmess://{vmess_b64}")).unwrap();
    assert_eq!(vm.mode, WireMode::Vmess);
}

#[test]
fn vless_roundtrip_header() {
    let id = Uuid::parse_str("f47e3452-6ea8-4f3a-9c2b-1a0b9c8d7e6f").unwrap();
    let addr = VlessAddress::Domain("httpbin.org".into());
    let b = vless_build_request(&id, 443, &addr, b"hello");
    match vless_parse_request(&b, Some(&id)) {
        VlessParseResult::Ok {
            port,
            payload,
            uuid_ok,
            ..
        } => {
            assert!(uuid_ok);
            assert_eq!(port, 443);
            assert_eq!(payload, b"hello");
        }
        _ => panic!("unexpected vless parse"),
    }
}

#[test]
fn vmess_chunk_roundtrip() {
    let id = Uuid::parse_str("f47e3452-6ea8-4f3a-9c2b-1a0b9c8d7e6f").unwrap();
    let ct = vmess_aead_encode_chunk(&id, b"payload").unwrap();
    let (plain, n) = vmess_aead_decode_chunk(&id, &ct).unwrap().unwrap();
    assert_eq!(plain, b"payload");
    assert_eq!(n, ct.len());
}

#[test]
fn vmess_open_header_decrypt() {
    let id = Uuid::parse_str("f47e3452-6ea8-4f3a-9c2b-1a0b9c8d7e6f").unwrap();
    let addr = VlessAddress::Domain("example.com".into());
    let h = vmess_client_header(&id, 80, &addr).unwrap();
    let open = vmess_server_open_header(&id, &h).unwrap().unwrap();
    assert_eq!(open.port, 80);
    match &open.addr {
        VlessAddress::Domain(s) => assert_eq!(s, "example.com"),
        _ => panic!("expected domain"),
    }
}
