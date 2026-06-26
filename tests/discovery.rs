use charles_mcp::web::discovery::{DiscoveredEndpoints, discover_from_html};

fn discover() -> DiscoveredEndpoints {
    discover_from_html(include_str!("fixtures/control_page.html"))
}

#[test]
fn finds_export_with_formats() {
    let d = discover();
    let export = d.export.expect("export endpoint discovered");
    assert!(!export.path.is_empty(), "export path should be non-empty");
    assert_eq!(export.method, "POST");
    assert_eq!(export.format_field.as_deref(), Some("format"));
    assert!(
        export.formats.contains(&"chlsj".to_string()),
        "formats should include chlsj, got {:?}",
        export.formats
    );
    assert!(export.formats.contains(&"har".to_string()));
}

#[test]
fn finds_download_chls() {
    let d = discover();
    let dl = d.download_chls.expect("download endpoint discovered");
    assert!(dl.path.contains("download"));
}

#[test]
fn finds_clear() {
    let d = discover();
    let clear = d.clear.expect("clear endpoint discovered");
    assert!(clear.path.contains("clear"));
}

#[test]
fn finds_quit() {
    let d = discover();
    let quit = d.quit.expect("quit endpoint discovered");
    assert!(quit.path.contains("quit"));
}

#[test]
fn export_is_not_confused_with_clear() {
    let d = discover();
    // The clear form must not be selected as export.
    let export = d.export.unwrap();
    assert!(!export.path.contains("clear"));
}

#[test]
fn empty_html_yields_all_none() {
    assert_eq!(discover_from_html(""), DiscoveredEndpoints::default());
    assert_eq!(
        discover_from_html("   \n  "),
        DiscoveredEndpoints::default()
    );
}
