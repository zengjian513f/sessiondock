use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn node_id_is_minted_once_with_private_mode_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data").join("node-id");
    let first = node_id(&path).unwrap();
    assert!(is_node_id(&first), "{first}");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), format!("{first}\n"));
    assert_eq!(
        node_id(&path).unwrap(),
        first,
        "an existing file is read, never rewritten"
    );
}

#[test]
fn node_id_file_with_other_content_is_rejected_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node-id");
    fs::write(&path, "not-a-node-id\n").unwrap();
    let error = node_id(&path).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(fs::read_to_string(&path).unwrap(), "not-a-node-id\n");
    fs::write(&path, format!("  {}\n", "A".repeat(32))).unwrap();
    assert!(node_id(&path).is_err(), "uppercase hex is not a node id");
    fs::write(&path, format!("  {}\n", "c".repeat(32))).unwrap();
    assert_eq!(
        node_id(&path).unwrap(),
        "c".repeat(32),
        "surrounding whitespace is stripped"
    );
}

#[test]
fn token_grammar_matches_the_python_pattern() {
    assert!(is_token(&"a".repeat(32)));
    assert!(is_token(&"Z9._~+/=-".repeat(4)));
    assert!(is_token(&"a".repeat(256)));
    assert!(!is_token(&"a".repeat(31)));
    assert!(!is_token(&"a".repeat(257)));
    assert!(!is_token(&format!("{} ", "a".repeat(32))));
    assert!(!is_token(&format!("{}!", "a".repeat(32))));
    assert!(!is_token(&format!("{}é", "a".repeat(32))));
    assert!(!is_token(""));
}

#[test]
fn token_file_is_stripped_validated_and_compared_in_constant_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    fs::write(&path, format!("\n {}\t\n", "s3cret-".repeat(8))).unwrap();
    let token = NodeToken::load(&path).unwrap();
    assert_eq!(token.as_str(), "s3cret-".repeat(8));
    assert!(token.verify(&"s3cret-".repeat(8)));
    assert!(!token.verify(&"s3cret-".repeat(7)));
    assert!(!token.verify(&format!("{}x", "s3cret-".repeat(8))));
    assert!(!token.verify(""));
    assert_eq!(format!("{token:?}"), "NodeToken(<redacted>)");

    fs::write(&path, "short\n").unwrap();
    assert_eq!(
        NodeToken::load(&path).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(NodeToken::load(&dir.path().join("missing")).is_err());
}
