use driver::clang_discovery::resolve_llvm_link;

#[test]
fn finds_llvm_link_beside_clang() {
    let dir = std::env::temp_dir().join("epiccc_llvmlink_ok");
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join("llvm-link");
    std::fs::write(&link, b"").unwrap();
    let found = resolve_llvm_link(&dir.join("clang")).unwrap();
    assert_eq!(found, link);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reports_a_clean_error_when_missing() {
    let dir = std::env::temp_dir().join("epiccc_llvmlink_missing");
    std::fs::create_dir_all(&dir).unwrap();
    let e = resolve_llvm_link(&dir.join("clang")).unwrap_err();
    assert!(e.contains("llvm-link"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}
