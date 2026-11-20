use gpui_goo::protocol::{Node, Selector, unique};

fn node(id: &str) -> Node {
    Node {
        path: vec![id.into()],
        role: None,
        label: None,
        value: None,
        checked: None,
        visible: true,
        disabled: None,
        bounds: [0.0; 4],
    }
}

#[test]
fn id_selector_requires_unique_match() {
    let selector = Selector::id("toggle");
    let nodes = [node("toggle"), node("other")];
    assert_eq!(
        unique(&nodes, &selector).unwrap().path.last().unwrap(),
        "toggle"
    );
    let duplicates = [node("toggle"), node("toggle")];
    assert_eq!(unique(&duplicates, &selector).unwrap_err(), "ambiguous");
}

#[test]
fn selector_state_distinguishes_missing_and_ambiguous() {
    assert_eq!(
        unique(&[node("other")], &Selector::id("toggle")).unwrap_err(),
        "not_found"
    );
    assert_eq!(
        unique(
            &[node("same"), node("same")],
            &Selector {
                id: None,
                role: None,
                label: None
            }
        )
        .unwrap_err(),
        "not_found"
    );
}
