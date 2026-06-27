use charles_mcp::web::discovery::{DiscoveredEndpoints, discover_from_html};

fn root() -> DiscoveredEndpoints {
    discover_from_html(include_str!("fixtures/control_page.html"))
}

fn session() -> DiscoveredEndpoints {
    discover_from_html(include_str!("fixtures/control_session_page.html"))
}

#[test]
fn real_root_page_exposes_quit_and_no_session_ops() {
    let d = root();
    assert!(
        d.quit
            .expect("quit link on the root page")
            .path
            .contains("quit"),
        "root page should expose quit"
    );
    assert!(
        d.export.is_none(),
        "export is on the session/ subpage, not the root"
    );
    assert!(
        d.clear.is_none(),
        "clear is on the session/ subpage, not the root"
    );
    assert!(
        d.download_chls.is_none(),
        "download is on the session/ subpage, not the root"
    );
}

#[test]
fn real_session_subpage_exposes_clear_export_download() {
    let d = session();
    assert!(d.clear.expect("clear link").path.contains("clear"));
    assert!(
        d.download_chls
            .expect("download link")
            .path
            .contains("download")
    );
    let export = d.export.expect("an export link");
    assert!(export.path.contains("export"), "got {:?}", export.path);
    assert!(
        !export.path.starts_with("session/"),
        "subpage links are relative to /session/ (no prefix); fetch_session_in_format \
         hardcodes the confirmed session/export-* paths instead of trusting this scrape"
    );
}

#[test]
fn empty_html_yields_all_none() {
    assert_eq!(discover_from_html(""), DiscoveredEndpoints::default());
    assert_eq!(
        discover_from_html("   \n  "),
        DiscoveredEndpoints::default()
    );
}
