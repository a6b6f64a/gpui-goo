use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub path: Vec<String>,
    pub role: Option<String>,
    pub label: Option<String>,
    pub value: Option<String>,
    pub checked: Option<bool>,
    pub visible: bool,
    pub disabled: Option<bool>,
    pub bounds: [f32; 4],
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    pub id: Option<String>,
    pub role: Option<String>,
    pub label: Option<String>,
}

impl Selector {
    pub fn id(id: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            role: None,
            label: None,
        }
    }

    pub fn matches(&self, node: &Node) -> bool {
        (self.id.is_some() || self.role.is_some() || self.label.is_some())
            && self
                .id
                .as_ref()
                .is_none_or(|id| node.path.last() == Some(id))
            && self
                .role
                .as_ref()
                .is_none_or(|role| node.role.as_ref() == Some(role))
            && self
                .label
                .as_ref()
                .is_none_or(|label| node.label.as_ref() == Some(label))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Snapshot,
    Click { selector: Selector },
    Scroll { selector: Selector, delta: [f32; 2] },
    Input { text: String },
    Press { key: String },
    Screenshot,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u8,
    pub token: String,
    pub command: Command,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Nodes { nodes: Vec<Node> },
    Done,
    Png { base64: String },
    Error { code: String, message: String },
}

pub fn unique<'a>(nodes: &'a [Node], selector: &Selector) -> Result<&'a Node, &'static str> {
    let mut found = nodes.iter().filter(|node| selector.matches(node));
    let first = found.next().ok_or("not_found")?;
    if found.next().is_some() {
        return Err("ambiguous");
    }
    Ok(first)
}
