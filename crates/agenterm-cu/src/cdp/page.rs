//! The background-tab verbs over one CDP session: `page text`, `page
//! find`, `page click`, `page hover`, `page scroll`, `page fill`, `page
//! nav`, `page screenshot`.
//!
//! Every function here is generic over `Transport` so the message shaping
//! and the verification logic run against fake transcripts in tests. The
//! actuators are split into `plan_*` (reads only) and `perform_*` (the
//! dispatch plus the read-back) so the executor can reserve a receipt in
//! between. Nothing here activates a target: focus emulation
//! (`Emulation.setFocusEmulationEnabled`) makes an unfocused page accept
//! text input without bringing it to the front, and it is switched off
//! again afterwards.

use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

use super::ax::{self, AxQuery};
use super::targets::{PageTarget, TargetSelector};
use super::ws::{Session, Transport};
use super::{
    CdpError,
    evaluate::{evaluate_on, evaluate_on_await},
};

/// How many matches `page find` describes (boxes, paths) per call.
pub const MAX_FIND: usize = 20;
/// How many candidates an ambiguity error lists.
pub const MAX_CANDIDATES: usize = 10;
pub const DEFAULT_NAV_WAIT_MS: u64 = 10_000;
pub const MAX_NAV_WAIT_MS: u64 = 120_000;
pub const MAX_FILL_BYTES: usize = 64 * 1024;
/// Viewport coordinates are CSS pixels. This rejects infinities and
/// obviously accidental magnitudes before serde/CDP ever sees them.
pub const MAX_POINTER_COORD: f64 = 1_000_000.0;
/// Bound one CDP wheel request while retaining high-resolution deltas.
pub const MAX_SCROLL_DELTA: f64 = 1_000_000.0;
static HOVER_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static SCROLL_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// The identity fields every reply of these verbs carries.
pub struct Ctx {
    pub port: u16,
    pub target: PageTarget,
    pub selector: TargetSelector,
}

impl Ctx {
    fn envelope(&self, via: &str) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        map.insert("addressing".into(), json!("cdp-target"));
        map.insert("mechanism".into(), json!("cdp"));
        map.insert("backend".into(), json!("cdp"));
        map.insert("port".into(), json!(self.port));
        map.insert("via".into(), json!(via));
        map.insert("target".into(), self.target.identity_json());
        map.insert("selector".into(), self.selector.json());
        map.insert("focus_changed".into(), json!(false));
        map
    }
}

fn with(mut map: serde_json::Map<String, Value>, extra: Value) -> Value {
    if let Some(more) = extra.as_object() {
        for (key, value) in more {
            map.insert(key.clone(), value.clone());
        }
    }
    Value::Object(map)
}

// ---------------------------------------------------------------- scripts
//
// Function declarations the browser runs on one node (`Runtime.callFunctionOn`)
// or the document (`Runtime.evaluate`). They are data sent over CDP; this
// process never constructs a function from them.

/// Describe one node: a selector-ish path, tag, text, role, name, value,
/// attributes and whether it takes text.
const DESCRIBE_FN: &str = r#"function() {
  const el = this.nodeType === 3 ? this.parentElement : this;
  const parts = [];
  let cur = el;
  while (cur && cur.nodeType === 1 && parts.length < 8) {
    let s = cur.tagName.toLowerCase();
    if (cur.id) { parts.unshift(s + '#' + cur.id); break; }
    const p = cur.parentElement;
    if (p) {
      const same = Array.from(p.children).filter(c => c.tagName === cur.tagName);
      if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(cur) + 1) + ')';
    }
    parts.unshift(s);
    cur = p;
  }
  const text = this.nodeType === 3 ? (this.nodeValue || '') : (el ? (el.innerText || el.textContent || '') : '');
  const attrs = el ? el.getAttributeNames().map(n => n + '=' + el.getAttribute(n)).join(' ') : '';
  const editable = !!(el && (el.isContentEditable || ((el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && !el.disabled && !el.readOnly)));
  return {
    path: parts.join(' > '),
    tag: el ? el.tagName.toLowerCase() : '#text',
    text: text.trim().slice(0, 500),
    role: el ? el.getAttribute('role') : null,
    name: el ? (el.getAttribute('aria-label') || el.getAttribute('name') || el.getAttribute('placeholder') || null) : null,
    value: (el && el.value !== undefined) ? String(el.value) : null,
    checked: (el && typeof el.checked === 'boolean') ? el.checked : null,
    attrs: attrs.slice(0, 500),
    editable: editable,
    text_node: this.nodeType === 3
  };
}"#;

/// The document state a click is verified against.
const DOC_STATE_EXPR: &str = r#"(function() {
  const a = document.activeElement;
  return {
    url: location.href,
    title: document.title,
    ready: document.readyState,
    text_len: document.body ? (document.body.innerText || '').length : 0,
    html_len: document.documentElement ? document.documentElement.outerHTML.length : 0,
    active: a ? (a.tagName.toLowerCase() + (a.id ? '#' + a.id : '')) : null,
    has_focus: document.hasFocus()
  };
})()"#;

fn point_state_expr(x: f64, y: f64) -> String {
    format!(
        r#"(function() {{
  const at = document.elementFromPoint({x}, {y});
  const path = (el) => {{
    if (!el || el.nodeType !== 1) return null;
    const parts = [];
    let cur = el;
    while (cur && cur.nodeType === 1 && parts.length < 12) {{
      let s = cur.tagName.toLowerCase();
      if (cur.id) {{ s += '#' + CSS.escape(cur.id); parts.unshift(s); break; }}
      const p = cur.parentElement;
      if (p) {{
        const same = Array.from(p.children).filter(c => c.tagName === cur.tagName);
        if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(cur) + 1) + ')';
      }}
      parts.unshift(s); cur = p;
    }}
    return parts.join(' > ');
  }};
  let scroll = at;
  while (scroll && scroll !== document.documentElement) {{
    const style = getComputedStyle(scroll);
    const canX = /(auto|scroll|overlay)/.test(style.overflowX) && scroll.scrollWidth > scroll.clientWidth;
    const canY = /(auto|scroll|overlay)/.test(style.overflowY) && scroll.scrollHeight > scroll.clientHeight;
    if (canX || canY) break;
    scroll = scroll.parentElement;
  }}
  scroll = scroll || document.scrollingElement || document.documentElement;
  const hovered = Array.from(document.querySelectorAll(':hover')).pop() || null;
  return {{
    hit: path(at), hovered: path(hovered),
    scroll: {{ selector: path(scroll), left: scroll.scrollLeft, top: scroll.scrollTop,
      width: scroll.scrollWidth, height: scroll.scrollHeight,
      client_width: scroll.clientWidth, client_height: scroll.clientHeight }}
  }};
}})()"#
    )
}

fn hover_probe_install_expr(key: &str) -> String {
    let key = serde_json::to_string(key).unwrap_or_else(|_| "null".into());
    format!(
        r#"(function() {{
  const key = {key};
  const state = {{ count: 0, target: null, x: null, y: null }};
  const handler = function(e) {{
    state.count += 1; state.target = e.target;
    state.x = e.clientX; state.y = e.clientY;
  }};
  state.handler = handler; window[key] = state;
  window.addEventListener('mousemove', handler, {{ capture: true, once: true }});
  return true;
}})()"#
    )
}

fn hover_probe_read_expr(key: &str, x: f64, y: f64) -> String {
    let key = serde_json::to_string(key).unwrap_or_else(|_| "null".into());
    format!(
        r#"(function() {{
  const key = {key}; const state = window[key] || null;
  const hit = document.elementFromPoint({x}, {y});
  const out = state ? {{ count: state.count, target_matches_hit: state.target === hit,
    x: state.x, y: state.y }} : null;
  if (state && state.handler) window.removeEventListener('mousemove', state.handler, true);
  try {{ delete window[key]; }} catch (_) {{ window[key] = undefined; }}
  return out;
}})()"#
    )
}

fn scroll_probe_install_expr(key: &str, selector: &str) -> String {
    let key = serde_json::to_string(key).unwrap_or_else(|_| "null".into());
    let selector = serde_json::to_string(selector).unwrap_or_else(|_| "null".into());
    format!(
        r#"(function() {{
  const key = {key}; const el = document.querySelector({selector});
  if (!el) return false;
  const state = {{ count: 0, left: el.scrollLeft, top: el.scrollTop, resolve: null }};
  const handler = function() {{
    state.count += 1; state.left = el.scrollLeft; state.top = el.scrollTop;
    if (state.resolve) state.resolve();
  }};
  state.element = el; state.handler = handler; window[key] = state;
  el.addEventListener('scroll', handler, {{ capture: true, once: true }});
  return true;
}})()"#
    )
}

fn scroll_probe_read_expr(key: &str, selector: &str) -> String {
    let key = serde_json::to_string(key).unwrap_or_else(|_| "null".into());
    let selector = serde_json::to_string(selector).unwrap_or_else(|_| "null".into());
    format!(
        r#"(async function() {{
  const key = {key}; const state = window[key] || null;
  if (state && state.count === 0) {{
    await new Promise(resolve => {{
      const timeout = setTimeout(resolve, 1000);
      state.resolve = () => {{ clearTimeout(timeout); resolve(); }};
    }});
  }}
  const el = document.querySelector({selector});
  const out = el ? {{ selector: {selector}, left: el.scrollLeft, top: el.scrollTop,
    width: el.scrollWidth, height: el.scrollHeight,
    client_width: el.clientWidth, client_height: el.clientHeight,
    scroll_events: state ? state.count : 0 }} : null;
  if (state && state.element && state.handler) state.element.removeEventListener('scroll', state.handler, true);
  try {{ delete window[key]; }} catch (_) {{ window[key] = undefined; }}
  return out;
}})()"#
    )
}

/// Select everything in the node so the next `Input.insertText` replaces it.
const SELECT_ALL_FN: &str = r#"function() {
  if (typeof this.select === 'function') { this.select(); return 'select'; }
  if (this.isContentEditable) {
    const r = document.createRange(); r.selectNodeContents(this);
    const s = getSelection(); s.removeAllRanges(); s.addRange(r); return 'range';
  }
  return 'none';
}"#;

/// The node's current text: `value` for a field, text for contenteditable.
const VALUE_FN: &str = r#"function() {
  return this.value !== undefined ? String(this.value) : (this.textContent || '');
}"#;

/// Fallback reader when the Accessibility domain is unavailable: one row
/// per block-level element with words, in document order.
const DOM_TEXT_EXPR: &str = r#"(function() {
  const out = [];
  const block = new Set(['P','H1','H2','H3','H4','H5','H6','LI','TD','TH','BUTTON','A','LABEL','INPUT','TEXTAREA','SELECT','SUMMARY','DT','DD','PRE','BLOCKQUOTE','FIGCAPTION','OPTION','LEGEND','CAPTION']);
  const walk = (el) => {
    for (const c of el.children) {
      if (block.has(c.tagName)) {
        const field = c.tagName === 'INPUT' || c.tagName === 'TEXTAREA' || c.tagName === 'SELECT';
        const t = field ? (c.value || '') : (c.innerText || '').trim();
        const name = c.getAttribute('aria-label') || c.getAttribute('placeholder') || null;
        if (t || name) out.push({ role: c.tagName.toLowerCase(), text: t.slice(0, 2000), name: name, editable: field && !c.disabled && !c.readOnly });
        if (out.length >= 5000) return;
      } else {
        walk(c);
      }
    }
  };
  walk(document.body || document.documentElement);
  return out;
})()"#;

// ------------------------------------------------------------------ nodes

/// How the actuators name one node.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeQuery {
    Css(String),
    Text(String),
    Role { role: String, name: Option<String> },
    Node(u64),
}

impl NodeQuery {
    pub fn json(&self) -> Value {
        match self {
            Self::Css(css) => json!({ "selector": css }),
            Self::Text(text) => json!({ "text": text }),
            Self::Role { role, name } => json!({ "role": role, "name": name }),
            Self::Node(node) => json!({ "node": node }),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Css(css) => format!("--selector {css:?}"),
            Self::Text(text) => format!("--text {text:?}"),
            Self::Role { role, name: None } => format!("--role {role:?}"),
            Self::Role {
                role,
                name: Some(name),
            } => format!("--role {role:?} --name {name:?}"),
            Self::Node(node) => format!("--node {node}"),
        }
    }
}

/// A node's layout box in CSS pixels of the main frame's viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NodeBox {
    /// From a `DOM.getBoxModel` result: the content quad's bounding box.
    pub fn from_model(model: &Value) -> Option<Self> {
        let quad = model["model"]["content"].as_array()?;
        if quad.len() < 8 {
            return None;
        }
        let numbers: Vec<f64> = quad.iter().filter_map(Value::as_f64).collect();
        if numbers.len() < 8 {
            return None;
        }
        let xs = [numbers[0], numbers[2], numbers[4], numbers[6]];
        let ys = [numbers[1], numbers[3], numbers[5], numbers[7]];
        let min_x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_y = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Some(Self {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        })
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn json(&self) -> Value {
        json!({ "x": self.x, "y": self.y, "width": self.width, "height": self.height })
    }
}

/// One resolved node with everything a caller needs to pick it again.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundNode {
    pub node: u64,
    pub ax_id: Option<String>,
    pub ax_role: Option<String>,
    pub ax_name: Option<String>,
    /// `DESCRIBE_FN` output (`null` when the node could not be described).
    pub described: Value,
    pub node_box: Option<NodeBox>,
}

impl FoundNode {
    pub fn json(&self) -> Value {
        let described = &self.described;
        json!({
            "node": self.node,
            "path": described["path"],
            "tag": described["tag"],
            "role": self.ax_role.clone().map(Value::from).unwrap_or(described["role"].clone()),
            "name": self.ax_name.clone().map(Value::from).unwrap_or(described["name"].clone()),
            "text": described["text"],
            "value": described["value"],
            "editable": described["editable"],
            "ax_id": self.ax_id,
            "box": self.node_box.map(|b| b.json()),
        })
    }

    pub fn is_editable(&self) -> bool {
        self.described["editable"].as_bool().unwrap_or(false)
    }
}

fn ensure_document<T: Transport>(session: &mut Session<T>) -> Result<u64, CdpError> {
    let document = session.call("DOM.getDocument", json!({ "depth": 0 }))?;
    document["root"]["nodeId"]
        .as_u64()
        .ok_or_else(|| CdpError::typed("unsupported", "CDP DOM.getDocument returned no root"))
}

/// Run `function_declaration` with the node as `this`, by value.
fn call_on_node<T: Transport>(
    session: &mut Session<T>,
    backend_node_id: u64,
    function_declaration: &str,
) -> Result<Value, CdpError> {
    let resolved = session.call(
        "DOM.resolveNode",
        json!({ "backendNodeId": backend_node_id }),
    )?;
    let Some(object_id) = resolved["object"]["objectId"].as_str() else {
        return Err(CdpError::typed(
            "cdp_node_not_found",
            format!(
                "CDP node {backend_node_id} has no JavaScript object (removed from the document?)"
            ),
        )
        .with_detail(json!({ "node": backend_node_id })));
    };
    let result = session.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": function_declaration,
            "returnByValue": true,
        }),
    )?;
    if let Some(exception) = result.get("exceptionDetails") {
        return Err(CdpError::typed(
            "unsupported",
            format!(
                "CDP callFunctionOn threw: {}",
                exception["exception"]["description"]
                    .as_str()
                    .or_else(|| exception["text"].as_str())
                    .unwrap_or("exception")
            ),
        ));
    }
    Ok(result["result"]["value"].clone())
}

fn node_box<T: Transport>(session: &mut Session<T>, backend_node_id: u64) -> Option<NodeBox> {
    session
        .call(
            "DOM.getBoxModel",
            json!({ "backendNodeId": backend_node_id }),
        )
        .ok()
        .and_then(|model| NodeBox::from_model(&model))
}

fn describe<T: Transport>(
    session: &mut Session<T>,
    backend_node_id: u64,
    ax: Option<&ax::AxNode>,
) -> FoundNode {
    let described = call_on_node(session, backend_node_id, DESCRIBE_FN).unwrap_or(Value::Null);
    let node_box = node_box(session, backend_node_id);
    FoundNode {
        node: backend_node_id,
        ax_id: ax.map(|node| node.id.clone()),
        ax_role: ax.map(|node| node.role.clone()),
        ax_name: ax.map(|node| node.name.clone()),
        described,
        node_box,
    }
}

/// One resolved backend node id with the AX node it came from (when the
/// query went through the AX tree).
type ResolvedId = (u64, Option<ax::AxNode>);

/// The backend node ids `query` names, in document order, plus the total
/// before the `MAX_FIND` cut. Zero is typed `cdp_node_not_found`.
fn resolve_ids<T: Transport>(
    session: &mut Session<T>,
    query: &NodeQuery,
) -> Result<(Vec<ResolvedId>, usize), CdpError> {
    let found: Vec<ResolvedId> = match query {
        NodeQuery::Node(id) => vec![(*id, None)],
        NodeQuery::Css(css) => {
            let root = ensure_document(session)?;
            let result = session
                .call(
                    "DOM.querySelectorAll",
                    json!({ "nodeId": root, "selector": css }),
                )
                .map_err(|error| {
                    if error.failed_method() == Some("DOM.querySelectorAll") {
                        let message = format!("CSS selector {css:?} was rejected by the document");
                        error.recode("invalid_input", message)
                    } else {
                        error
                    }
                })?;
            let node_ids: Vec<u64> = result["nodeIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .collect();
            let mut out = Vec::new();
            for node_id in node_ids.iter().take(MAX_FIND.max(MAX_CANDIDATES)) {
                let described = session.call("DOM.describeNode", json!({ "nodeId": node_id }))?;
                if let Some(backend) = described["node"]["backendNodeId"].as_u64() {
                    out.push((backend, None));
                }
            }
            // The cut is reported through `total`, so keep the true count.
            return Ok((out, node_ids.len()));
        }
        NodeQuery::Text(text) => {
            let tree = ax_tree(session)?;
            ax::find_nodes(&tree, &AxQuery::Text(text.clone()))
                .into_iter()
                .filter_map(|node| node.backend_node_id.map(|id| (id, Some(node.clone()))))
                .collect()
        }
        NodeQuery::Role { role, name } => {
            let tree = ax_tree(session)?;
            ax::find_nodes(
                &tree,
                &AxQuery::Role {
                    role: role.clone(),
                    name: name.clone(),
                },
            )
            .into_iter()
            .filter_map(|node| node.backend_node_id.map(|id| (id, Some(node.clone()))))
            .collect()
        }
    };
    let total = found.len();
    Ok((found, total))
}

fn ax_tree<T: Transport>(session: &mut Session<T>) -> Result<Vec<ax::AxNode>, CdpError> {
    let result = session.call("Accessibility.getFullAXTree", json!({}))?;
    Ok(ax::parse_tree(&result))
}

fn not_found(query: &NodeQuery) -> CdpError {
    CdpError::typed(
        "cdp_node_not_found",
        format!("no node matches {} in this page target", query.describe()),
    )
    .with_detail(json!({ "query": query.json() }))
}

/// `page find`: every match (bounded), described with a path and a box.
pub fn find<T: Transport>(
    session: &mut Session<T>,
    ctx: &Ctx,
    query: &NodeQuery,
) -> Result<Value, CdpError> {
    let (ids, total) = resolve_ids(session, query)?;
    if ids.is_empty() {
        return Err(not_found(query));
    }
    let via = match query {
        NodeQuery::Css(_) => "DOM.querySelectorAll",
        NodeQuery::Node(_) => "DOM.describeNode",
        _ => "Accessibility.getFullAXTree",
    };
    let nodes: Vec<Value> = ids
        .iter()
        .take(MAX_FIND)
        .map(|(id, ax)| describe(session, *id, ax.as_ref()).json())
        .collect();
    Ok(with(
        ctx.envelope(via),
        json!({
            "query": query.json(),
            "total": total,
            "returned": nodes.len(),
            "truncated": total > nodes.len(),
            "nodes": nodes,
            "next_actions": [
                "page click --node N (or the same --selector / --text) presses the box centre",
                "page fill --node N --text T writes a field; --clear replaces, --submit sends Enter",
            ],
        }),
    ))
}

/// Exactly one node for an actuator: zero is `cdp_node_not_found`, more
/// is `cdp_node_ambiguous` with the (bounded) candidates.
pub fn resolve_one<T: Transport>(
    session: &mut Session<T>,
    query: &NodeQuery,
) -> Result<FoundNode, CdpError> {
    let (ids, total) = resolve_ids(session, query)?;
    match ids.as_slice() {
        [] => Err(not_found(query)),
        [(id, ax)] => Ok(describe(session, *id, ax.as_ref())),
        many => {
            let candidates: Vec<Value> = many
                .iter()
                .take(MAX_CANDIDATES)
                .map(|(id, ax)| describe(session, *id, ax.as_ref()).json())
                .collect();
            Err(CdpError::typed(
                "cdp_node_ambiguous",
                format!(
                    "{total} nodes match {}; refusing to guess -- narrow the selector or pass --node",
                    query.describe()
                ),
            )
            .with_detail(json!({
                "query": query.json(),
                "count": total,
                "candidates": candidates,
            })))
        }
    }
}

// ------------------------------------------------------------------- text

/// `page text`: the AX tree's words in document order (`Accessibility.
/// getFullAXTree`), or a block-element `innerText` walk when that domain
/// is unavailable. Same row shape as the AX-tree verb, `backend: "cdp"`.
pub fn text<T: Transport>(
    session: &mut Session<T>,
    ctx: &Ctx,
    max_bytes: usize,
) -> Result<Value, CdpError> {
    match session.call("Accessibility.getFullAXTree", json!({})) {
        Ok(result) => {
            let tree = ax::parse_tree(&result);
            let reading = ax::text_rows(&tree, max_bytes);
            let mut next_actions = vec![
                "pick a row, then page click --node ID / page fill --node ID (the id is the backend DOM node id)".to_owned(),
            ];
            if reading.truncated {
                next_actions.push(format!(
                    "text cut at --max-bytes {max_bytes}; raise it (<= {})",
                    crate::page_text::MAX_MAX_BYTES
                ));
            }
            Ok(with(
                ctx.envelope("Accessibility.getFullAXTree"),
                json!({
                    "order": "document (AX tree pre-order)",
                    "max_bytes": max_bytes,
                    "ax_nodes": tree.len(),
                    "candidates": reading.candidates,
                    "merged": reading.merged,
                    "returned": reading.rows.len(),
                    "bytes": reading.bytes,
                    "truncated": reading.truncated,
                    "next_actions": next_actions,
                    "rows": reading.rows.iter().map(ax::TextRow::json).collect::<Vec<_>>(),
                }),
            ))
        }
        Err(error) if error.failed_method() == Some("Accessibility.getFullAXTree") => {
            let rows = evaluate_on(session, DOM_TEXT_EXPR)?;
            let mut out = Vec::new();
            let mut bytes = 0usize;
            let mut truncated = false;
            for (index, row) in rows.as_array().into_iter().flatten().enumerate() {
                let text = row["text"].as_str().unwrap_or_default();
                if bytes + text.len() > max_bytes {
                    truncated = true;
                    break;
                }
                bytes += text.len();
                let mut shaped = json!({
                    "id": format!("dom:{index}"),
                    "role": row["role"],
                    "text": text,
                });
                if let Some(name) = row["name"].as_str() {
                    shaped["name"] = json!(name);
                }
                if row["editable"].as_bool() == Some(true) {
                    shaped["editable"] = json!(true);
                }
                out.push(shaped);
            }
            Ok(with(
                ctx.envelope("Runtime.evaluate innerText walk"),
                json!({
                    "order": "document (block elements)",
                    "max_bytes": max_bytes,
                    "fallback": {
                        "reason": "Accessibility.getFullAXTree unavailable on this target",
                        "cdp_message": error.detail["cdp_message"],
                    },
                    "returned": out.len(),
                    "bytes": bytes,
                    "truncated": truncated,
                    "next_actions": [
                        "rows from the DOM walk carry no node id; use page find --selector / --text to pick a node",
                    ],
                    "rows": out,
                }),
            ))
        }
        Err(error) => Err(error),
    }
}

// ------------------------------------------------------------------ click

fn doc_state<T: Transport>(session: &mut Session<T>) -> Result<Value, CdpError> {
    evaluate_on(session, DOC_STATE_EXPR)
}

fn focus_emulation<T: Transport>(session: &mut Session<T>, enabled: bool) -> bool {
    session
        .call(
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": enabled }),
        )
        .is_ok()
}

/// Which document / node fields differ between two readings.
pub fn changed_fields(before: &Value, after: &Value, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .filter(|key| before[**key] != after[**key])
        .map(|key| (*key).to_owned())
        .collect()
}

const DOC_KEYS: &[&str] = &["url", "title", "ready", "text_len", "html_len", "active"];
const NODE_KEYS: &[&str] = &["text", "value", "checked", "attrs"];

#[derive(Debug)]
pub struct ClickPlan {
    pub node: FoundNode,
    pub center: (f64, f64),
    pub scrolled: bool,
    pub button: String,
    pub clicks: u32,
    pub before_doc: Value,
}

impl ClickPlan {
    pub fn json(&self) -> Value {
        json!({
            "node": self.node.json(),
            "at": { "x": self.center.0, "y": self.center.1 },
            "scrolled_into_view": self.scrolled,
            "button": self.button,
            "clicks": self.clicks,
            "before": self.before_doc,
        })
    }
}

/// Resolve the node, scroll it into view, and read the box centre and the
/// document state. Nothing is pressed.
pub fn plan_click<T: Transport>(
    session: &mut Session<T>,
    query: &NodeQuery,
    button: &str,
    clicks: u32,
) -> Result<ClickPlan, CdpError> {
    if !matches!(button, "left" | "right" | "middle") {
        return Err(CdpError::typed(
            "invalid_input",
            format!("page click --button accepts left | right | middle, got {button:?}"),
        ));
    }
    if !(1..=3).contains(&clicks) {
        return Err(CdpError::typed(
            "invalid_input",
            format!("page click --clicks accepts 1..=3, got {clicks}"),
        ));
    }
    let mut node = resolve_one(session, query)?;
    let scrolled = session
        .call(
            "DOM.scrollIntoViewIfNeeded",
            json!({ "backendNodeId": node.node }),
        )
        .is_ok();
    if scrolled {
        node.node_box = node_box(session, node.node);
    }
    let Some(node_box) = node.node_box else {
        return Err(CdpError::typed(
            "cdp_node_not_visible",
            format!(
                "node {} has no layout box (display:none, detached, or not rendered); nothing was clicked",
                node.node
            ),
        )
        .with_detail(json!({ "node": node.json(), "effect": "not_performed" })));
    };
    let before_doc = doc_state(session)?;
    Ok(ClickPlan {
        center: node_box.center(),
        node,
        scrolled,
        button: button.to_owned(),
        clicks,
        before_doc,
    })
}

#[derive(Debug)]
pub struct ActuationOutcome {
    pub performed: bool,
    pub verified: bool,
    pub payload: Value,
}

/// Dispatch the mouse events of `plan` and read the document and the node
/// back. `verified` means something observable changed.
pub fn perform_click<T: Transport>(
    session: &mut Session<T>,
    ctx: &Ctx,
    plan: &ClickPlan,
) -> Result<ActuationOutcome, CdpError> {
    let (x, y) = plan.center;
    let buttons = match plan.button.as_str() {
        "right" => 2,
        "middle" => 4,
        _ => 1,
    };
    let emulated = focus_emulation(session, true);
    let mut dispatched = 0u32;
    let mut mechanism_error = None;
    let moved = session.call(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none" }),
    );
    if let Err(error) = moved {
        mechanism_error = Some(error);
    } else {
        for click in 1..=plan.clicks {
            for kind in ["mousePressed", "mouseReleased"] {
                let result = session.call(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": kind,
                        "x": x,
                        "y": y,
                        "button": plan.button,
                        "buttons": if kind == "mousePressed" { buttons } else { 0 },
                        "clickCount": click,
                    }),
                );
                match result {
                    Ok(_) => dispatched += 1,
                    Err(error) => {
                        mechanism_error = Some(error);
                        break;
                    }
                }
            }
            if mechanism_error.is_some() {
                break;
            }
        }
    }
    let performed = mechanism_error.is_none() && dispatched == plan.clicks * 2;
    let after_doc = doc_state(session).unwrap_or(Value::Null);
    let after_node = call_on_node(session, plan.node.node, DESCRIBE_FN).ok();
    if emulated {
        focus_emulation(session, false);
    }
    let mut changed = changed_fields(&plan.before_doc, &after_doc, DOC_KEYS);
    match &after_node {
        Some(after) => changed.extend(
            changed_fields(&plan.node.described, after, NODE_KEYS)
                .into_iter()
                .map(|key| format!("node.{key}")),
        ),
        None => changed.push("node.gone".into()),
    }
    let verified = performed && !changed.is_empty();
    let reason = if let Some(error) = &mechanism_error {
        Some(error.message.clone())
    } else if !performed {
        Some("dispatch_incomplete".to_owned())
    } else if changed.is_empty() {
        Some("no_observable_change".to_owned())
    } else {
        None
    };
    let payload = with(
        ctx.envelope("Input.dispatchMouseEvent"),
        json!({
            "action": "click",
            "node": plan.node.json(),
            "at": { "x": x, "y": y },
            "button": plan.button,
            "clicks": plan.clicks,
            "scrolled_into_view": plan.scrolled,
            "focus_emulation": if emulated { "enabled during the click, disabled after" } else { "unavailable" },
            "events_dispatched": dispatched + u32::from(moved_ok(&mechanism_error, dispatched)),
            "performed": performed,
            "verified": verified,
            "verification": {
                "method": "document-and-node-readback",
                "changed": changed,
                "reason": reason,
            },
            "before": plan.before_doc,
            "after": after_doc,
            "after_node": after_node,
        }),
    );
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(json!({ "receipt": payload })));
    }
    Ok(ActuationOutcome {
        performed,
        verified,
        payload,
    })
}

fn moved_ok(error: &Option<CdpError>, dispatched: u32) -> bool {
    error.is_none() || dispatched > 0
}

// ---------------------------------------------------------- hover / scroll

pub fn validate_pointer_coordinate(flag: &str, value: f64) -> Result<f64, String> {
    if !value.is_finite() || !(0.0..=MAX_POINTER_COORD).contains(&value) {
        return Err(format!(
            "{flag} must be a finite viewport CSS coordinate in 0..={MAX_POINTER_COORD}"
        ));
    }
    Ok(value)
}

pub fn validate_scroll_delta(flag: &str, value: f64) -> Result<f64, String> {
    if !value.is_finite() || value.abs() > MAX_SCROLL_DELTA {
        return Err(format!(
            "{flag} must be finite and within -{MAX_SCROLL_DELTA}..={MAX_SCROLL_DELTA}"
        ));
    }
    Ok(value)
}

#[derive(Debug)]
pub struct PointPlan {
    pub x: f64,
    pub y: f64,
    pub before: Value,
}

impl PointPlan {
    pub fn json(&self) -> Value {
        json!({ "at": { "x": self.x, "y": self.y }, "before": self.before })
    }
}

/// Read the element and nearest scrollable container at one viewport point.
/// This is the read-only half of both pointer actuators.
pub fn plan_point<T: Transport>(
    session: &mut Session<T>,
    x: f64,
    y: f64,
) -> Result<PointPlan, CdpError> {
    validate_pointer_coordinate("--x", x)
        .and_then(|_| validate_pointer_coordinate("--y", y))
        .map_err(|message| CdpError::typed("invalid_input", message))?;
    let before = evaluate_on(session, &point_state_expr(x, y))?;
    if before["hit"].as_str().is_none() {
        return Err(CdpError::typed(
            "cdp_point_not_found",
            format!("no rendered page element exists at viewport point ({x}, {y})"),
        )
        .with_detail(json!({ "at": { "x": x, "y": y }, "effect": "not_performed" })));
    }
    Ok(PointPlan { x, y, before })
}

/// Move the page pointer and verify the trusted event target/coordinates
/// against the element currently hit at the requested point.
pub fn perform_hover<T: Transport>(
    session: &mut Session<T>,
    ctx: &Ctx,
    plan: &PointPlan,
) -> Result<ActuationOutcome, CdpError> {
    let probe_key = format!(
        "__agenterm_cu_hover_{}_{}",
        std::process::id(),
        HOVER_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    evaluate_on(session, &hover_probe_install_expr(&probe_key))?;
    let emulated = focus_emulation(session, true);
    let dispatched = session.call(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseMoved", "x": plan.x, "y": plan.y, "button": "none" }),
    );
    if let Err(error) = dispatched {
        let _ = evaluate_on(session, &hover_probe_read_expr(&probe_key, plan.x, plan.y));
        if emulated {
            focus_emulation(session, false);
        }
        return Err(error);
    }
    let event = evaluate_on(session, &hover_probe_read_expr(&probe_key, plan.x, plan.y));
    let after = evaluate_on(session, &point_state_expr(plan.x, plan.y));
    if emulated {
        focus_emulation(session, false);
    }
    let event = event?;
    let after = after?;
    let hit = after["hit"].as_str();
    let hovered = after["hovered"].as_str();
    let event_verified = event["count"].as_u64().is_some_and(|count| count > 0)
        && event["target_matches_hit"] == true
        && event["x"]
            .as_f64()
            .is_some_and(|x| (x - plan.x).abs() <= 1.0)
        && event["y"]
            .as_f64()
            .is_some_and(|y| (y - plan.y).abs() <= 1.0);
    let css_verified = hit.is_some() && hit == hovered;
    let verified = event_verified;
    let payload = with(
        ctx.envelope("Input.dispatchMouseEvent"),
        json!({
            "action": "hover",
            "at": { "x": plan.x, "y": plan.y },
            "performed": true,
            "verified": verified,
            "focus_emulation": if emulated { "enabled during hover, disabled after" } else { "unavailable" },
            "verification": {
                "method": "trusted-mousemove-target-readback",
                "expected": { "target": hit, "x": plan.x, "y": plan.y },
                "observed": event,
                "css_hovered": hovered,
                "css_hover_matched": css_verified,
                "reason": if verified { Value::Null } else { json!("mousemove_not_observed_at_target") },
            },
            "before": plan.before,
            "after": after,
        }),
    );
    Ok(ActuationOutcome {
        performed: true,
        verified,
        payload,
    })
}

/// Dispatch a wheel event at one viewport point and read the exact scrollable
/// container selected during planning back. An edge that cannot move is
/// performed but deliberately unverified.
pub fn perform_scroll<T: Transport>(
    session: &mut Session<T>,
    ctx: &Ctx,
    plan: &PointPlan,
    delta_x: f64,
    delta_y: f64,
) -> Result<ActuationOutcome, CdpError> {
    validate_scroll_delta("--dx", delta_x)
        .and_then(|_| validate_scroll_delta("--dy", delta_y))
        .map_err(|message| CdpError::typed("invalid_input", message))?;
    if delta_x == 0.0 && delta_y == 0.0 {
        return Err(CdpError::typed(
            "invalid_input",
            "page scroll requires a non-zero --dx or --dy",
        ));
    }
    let selector = plan.before["scroll"]["selector"]
        .as_str()
        .ok_or_else(|| {
            CdpError::typed(
                "cdp_scroll_container_not_found",
                "no readable scroll container exists at the requested point",
            )
        })?
        .to_owned();
    let probe_key = format!(
        "__agenterm_cu_scroll_{}_{}",
        std::process::id(),
        SCROLL_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let probe_installed = evaluate_on(session, &scroll_probe_install_expr(&probe_key, &selector))?
        .as_bool()
        .unwrap_or(false);
    if !probe_installed {
        return Err(CdpError::typed(
            "cdp_scroll_container_not_found",
            "the planned scroll container disappeared before dispatch",
        ));
    }
    let emulated = focus_emulation(session, true);
    let dispatched = session.call(
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseWheel", "x": plan.x, "y": plan.y,
            "deltaX": delta_x, "deltaY": delta_y,
            "button": "none", "buttons": 0,
        }),
    );
    if let Err(error) = dispatched {
        let _ = evaluate_on_await(session, &scroll_probe_read_expr(&probe_key, &selector));
        if emulated {
            focus_emulation(session, false);
        }
        return Err(error);
    }
    let before_scroll = &plan.before["scroll"];
    let after_scroll = evaluate_on_await(session, &scroll_probe_read_expr(&probe_key, &selector));
    if emulated {
        focus_emulation(session, false);
    }
    let after_scroll = after_scroll?;
    let changed = changed_fields(before_scroll, &after_scroll, &["left", "top"]);
    let verified = !changed.is_empty();
    let after = json!({ "point": evaluate_on(session, &point_state_expr(plan.x, plan.y)).ok(), "scroll": after_scroll });
    let payload = with(
        ctx.envelope("Input.dispatchMouseEvent"),
        json!({
            "action": "scroll",
            "at": { "x": plan.x, "y": plan.y },
            "delta": { "x": delta_x, "y": delta_y },
            "performed": true,
            "verified": verified,
            "focus_emulation": if emulated { "enabled during scroll, disabled after" } else { "unavailable" },
            "verification": {
                "method": "scroll-container-offset-readback",
                "container": selector,
                "changed": changed,
                "reason": if verified { Value::Null } else { json!("no_observable_scroll_change") },
            },
            "before": plan.before,
            "after": after,
        }),
    );
    Ok(ActuationOutcome {
        performed: true,
        verified,
        payload,
    })
}

// ------------------------------------------------------------------- fill

#[derive(Debug)]
pub struct FillPlan {
    pub node: FoundNode,
    pub text: String,
    pub clear: bool,
    pub submit: bool,
    pub before_value: String,
    pub before_doc: Value,
}

impl FillPlan {
    pub fn expected(&self) -> Option<String> {
        self.clear.then(|| self.text.clone())
    }

    pub fn json(&self) -> Value {
        json!({
            "node": self.node.json(),
            "text_bytes": self.text.len(),
            "clear": self.clear,
            "submit": self.submit,
            "before_value": self.before_value,
            "before": self.before_doc,
        })
    }
}

/// Resolve the field and read its current value. Nothing is written.
pub fn plan_fill<T: Transport>(
    session: &mut Session<T>,
    query: &NodeQuery,
    text: &str,
    clear: bool,
    submit: bool,
) -> Result<FillPlan, CdpError> {
    if text.len() > MAX_FILL_BYTES {
        return Err(CdpError::typed(
            "invalid_input",
            format!("page fill --text must be at most {MAX_FILL_BYTES} bytes"),
        ));
    }
    if text.is_empty() && !clear {
        return Err(CdpError::typed(
            "invalid_input",
            "page fill --text is empty; pass --clear to empty the field on purpose",
        ));
    }
    let node = resolve_one(session, query)?;
    if !node.is_editable() {
        return Err(CdpError::typed(
            "cdp_node_not_editable",
            format!(
                "node {} ({}) is not an enabled input, textarea or contenteditable; nothing was written",
                node.node,
                node.described["tag"].as_str().unwrap_or("?")
            ),
        )
        .with_detail(json!({ "node": node.json(), "effect": "not_performed" })));
    }
    let before_value = node.described["value"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            node.described["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        });
    let before_doc = doc_state(session)?;
    Ok(FillPlan {
        node,
        text: text.to_owned(),
        clear,
        submit,
        before_value,
        before_doc,
    })
}

/// Judge a fill read-back: with `--clear` the value must equal the text;
/// without it the text must have been inserted (the value grew by exactly
/// the text and contains it).
pub fn fill_verified(before: &str, text: &str, clear: bool, after: &str) -> bool {
    if clear {
        after == text
    } else {
        after.len() == before.len() + text.len() && after.contains(text)
    }
}

/// Focus the field, optionally select everything, insert the text, read
/// the value back, optionally press Enter.
pub fn perform_fill<T: Transport>(
    session: &mut Session<T>,
    ctx: &Ctx,
    plan: &FillPlan,
) -> Result<ActuationOutcome, CdpError> {
    let emulated = focus_emulation(session, true);
    let mut steps: Vec<Value> = Vec::new();
    let mut mechanism_error = None;
    let mut inserted = false;
    let focused = session.call("DOM.focus", json!({ "backendNodeId": plan.node.node }));
    match focused {
        Ok(_) => steps.push(json!({ "step": "DOM.focus", "ok": true })),
        Err(error) => mechanism_error = Some(error),
    }
    if mechanism_error.is_none() && plan.clear {
        match call_on_node(session, plan.node.node, SELECT_ALL_FN) {
            Ok(how) => steps.push(json!({ "step": "select-all", "ok": true, "how": how })),
            Err(error) => mechanism_error = Some(error),
        }
    }
    if mechanism_error.is_none() {
        match session.call("Input.insertText", json!({ "text": plan.text })) {
            Ok(_) => {
                inserted = true;
                steps.push(
                    json!({ "step": "Input.insertText", "ok": true, "bytes": plan.text.len() }),
                );
            }
            Err(error) => mechanism_error = Some(error),
        }
    }
    let after_value = call_on_node(session, plan.node.node, VALUE_FN)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned));
    let value_verified = after_value
        .as_deref()
        .is_some_and(|after| fill_verified(&plan.before_value, &plan.text, plan.clear, after));
    let mut submitted = None;
    if plan.submit && mechanism_error.is_none() {
        let key = json!({
            "key": "Enter",
            "code": "Enter",
            "windowsVirtualKeyCode": 13,
            "nativeVirtualKeyCode": 13,
        });
        let mut down = key.clone();
        down["type"] = json!("keyDown");
        down["text"] = json!("\r");
        down["unmodifiedText"] = json!("\r");
        let mut up = key;
        up["type"] = json!("keyUp");
        let sent = session
            .call("Input.dispatchKeyEvent", down)
            .and_then(|_| session.call("Input.dispatchKeyEvent", up));
        match sent {
            Ok(_) => {
                steps.push(json!({ "step": "Input.dispatchKeyEvent Enter", "ok": true }));
                submitted = Some(json!({ "dispatched": true }));
            }
            Err(error) => mechanism_error = Some(error),
        }
    }
    let after_doc = doc_state(session).unwrap_or(Value::Null);
    if emulated {
        focus_emulation(session, false);
    }
    let performed = inserted && mechanism_error.is_none();
    let verified = performed && value_verified;
    let reason = if let Some(error) = &mechanism_error {
        Some(error.message.clone())
    } else if !inserted {
        Some("insert_not_dispatched".to_owned())
    } else if after_value.is_none() {
        Some("value_unreadable".to_owned())
    } else if !value_verified {
        Some("value_mismatch".to_owned())
    } else {
        None
    };
    let payload = with(
        ctx.envelope("Input.insertText"),
        json!({
            "action": "fill",
            "node": plan.node.json(),
            "clear": plan.clear,
            "submit": plan.submit,
            "text_bytes": plan.text.len(),
            "focus_emulation": if emulated { "enabled during the write, disabled after" } else { "unavailable" },
            "steps": steps,
            "performed": performed,
            "verified": verified,
            "verification": {
                "method": "value-readback",
                "expected": plan.expected(),
                "before_value": plan.before_value,
                "after_value": after_value,
                "reason": reason,
            },
            "submitted": submitted,
            "before": plan.before_doc,
            "after": after_doc,
        }),
    );
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(json!({ "receipt": payload })));
    }
    Ok(ActuationOutcome {
        performed,
        verified,
        payload,
    })
}

// -------------------------------------------------------------------- nav

pub fn validate_nav_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("page nav --url must not be empty".into());
    }
    if trimmed.starts_with('-') {
        return Err(format!(
            "page nav --url {url:?} looks like a switch, not a URL"
        ));
    }
    if trimmed.contains('\n') || trimmed.contains('\0') {
        return Err("page nav --url must be one line".into());
    }
    if !trimmed.contains(':') {
        return Err(format!(
            "page nav --url {url:?} has no scheme (http:, https:, data:, file:, about:)"
        ));
    }
    Ok(())
}

pub fn validate_nav_wait(wait_ms: Option<u64>) -> Result<u64, String> {
    let wait = wait_ms.unwrap_or(DEFAULT_NAV_WAIT_MS);
    if wait > MAX_NAV_WAIT_MS {
        return Err(format!(
            "page nav --wait-ms must be at most {MAX_NAV_WAIT_MS}"
        ));
    }
    Ok(wait)
}

#[derive(Debug)]
pub struct NavPlan {
    pub url: String,
    pub wait_ms: u64,
    pub before_doc: Value,
}

impl NavPlan {
    pub fn json(&self) -> Value {
        json!({ "url": self.url, "wait_ms": self.wait_ms, "before": self.before_doc })
    }
}

/// Enable page events and read the document state. Nothing navigates.
pub fn plan_nav<T: Transport>(
    session: &mut Session<T>,
    url: &str,
    wait_ms: Option<u64>,
) -> Result<NavPlan, CdpError> {
    validate_nav_url(url).map_err(|message| CdpError::typed("invalid_input", message))?;
    let wait_ms =
        validate_nav_wait(wait_ms).map_err(|message| CdpError::typed("invalid_input", message))?;
    session.call("Page.enable", json!({}))?;
    let before_doc = doc_state(session)?;
    Ok(NavPlan {
        url: url.trim().to_owned(),
        wait_ms,
        before_doc,
    })
}

/// `Page.navigate` on this target (a background tab stays background),
/// then wait for `Page.loadEventFired` or the timeout.
pub fn perform_nav<T: Transport>(
    session: &mut Session<T>,
    ctx: &Ctx,
    plan: &NavPlan,
) -> Result<ActuationOutcome, CdpError> {
    let result = session.call("Page.navigate", json!({ "url": plan.url }))?;
    if let Some(error_text) = result["errorText"].as_str().filter(|text| !text.is_empty()) {
        return Err(CdpError::typed(
            "cdp_navigation_failed",
            format!("CDP Page.navigate to {:?} failed: {error_text}", plan.url),
        )
        .with_detail(json!({
            "url": plan.url,
            "errorText": error_text,
            "frameId": result["frameId"],
            "effect": "navigation_requested",
        })));
    }
    let started = std::time::Instant::now();
    let loaded = session
        .wait_event(
            "Page.loadEventFired",
            std::time::Duration::from_millis(plan.wait_ms),
        )?
        .is_some();
    let waited_ms = started.elapsed().as_millis() as u64;
    let after_doc = doc_state(session).unwrap_or(Value::Null);
    let ready = after_doc["ready"].as_str().unwrap_or_default().to_owned();
    let verified = loaded || (ready == "complete" && after_doc["url"] != plan.before_doc["url"]);
    let reason = if verified {
        None
    } else if !loaded {
        Some("load_timeout")
    } else {
        Some("not_complete")
    };
    let payload = with(
        ctx.envelope("Page.navigate"),
        json!({
            "action": "nav",
            "url": plan.url,
            "frameId": result["frameId"],
            "loaderId": result["loaderId"],
            "wait_ms": plan.wait_ms,
            "waited_ms": waited_ms,
            "performed": true,
            "verified": verified,
            "verification": {
                "method": "load-event",
                "load_event_fired": loaded,
                "ready_state": ready,
                "reason": reason,
            },
            "before": plan.before_doc,
            "after": after_doc,
            "final_url": after_doc["url"],
            "final_title": after_doc["title"],
        }),
    );
    Ok(ActuationOutcome {
        performed: true,
        verified,
        payload,
    })
}

// ------------------------------------------------------------- screenshot

/// `Page.captureScreenshot` as PNG bytes plus the reply metadata. With
/// `activate`, `Page.bringToFront` runs first (the only place this module
/// changes what is active) and the reply says `focus_changed: true`.
pub fn screenshot<T: Transport>(
    session: &mut Session<T>,
    ctx: &Ctx,
    activate: bool,
) -> Result<(Vec<u8>, Value), CdpError> {
    let mut envelope = ctx.envelope("Page.captureScreenshot");
    if activate {
        session.call("Page.bringToFront", json!({}))?;
        envelope.insert("focus_changed".into(), json!(true));
    }
    let result = session
        .call("Page.captureScreenshot", json!({ "format": "png" }))
        .map_err(|error| {
            if error.failed_method() == Some("Page.captureScreenshot") {
                let message = format!(
                    "Chromium refused to capture this target: {}; a background or occluded tab may not be painted, and this verb never activates it (pass --activate to bring it to the front deliberately)",
                    error.detail["cdp_message"].as_str().unwrap_or("unknown")
                );
                error
                    .recode("cdp_screenshot_unavailable", message)
                    .with_detail(json!({ "activate": activate }))
            } else {
                error
            }
        })?;
    let data = result["data"].as_str().unwrap_or_default();
    let bytes = base64_decode(data).map_err(|reason| {
        CdpError::typed(
            "unsupported",
            format!("CDP screenshot data is not base64: {reason}"),
        )
    })?;
    if bytes.is_empty() {
        return Err(CdpError::typed(
            "cdp_screenshot_unavailable",
            "CDP returned an empty screenshot",
        ));
    }
    let meta = with(
        envelope,
        json!({
            "format": "png",
            "bytes": bytes.len(),
            "activated": activate,
        }),
    );
    Ok((bytes, meta))
}

/// Standard base64 (with or without padding) to bytes.
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    fn value(byte: u8) -> Result<u32, String> {
        match byte {
            b'A'..=b'Z' => Ok((byte - b'A') as u32),
            b'a'..=b'z' => Ok((byte - b'a') as u32 + 26),
            b'0'..=b'9' => Ok((byte - b'0') as u32 + 52),
            b'+' | b'-' => Ok(62),
            b'/' | b'_' => Ok(63),
            other => Err(format!("byte {other:#x} is not base64")),
        }
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for byte in text.bytes() {
        if byte == b'=' || byte == b'\n' || byte == b'\r' {
            continue;
        }
        acc = (acc << 6) | value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::ws::fake;
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            port: 9222,
            target: PageTarget {
                id: "B2".into(),
                url: "data:text/html,B".into(),
                title: "cu-smoke-B".into(),
                kind: "page".into(),
                attached: Some(false),
                ws_url: Some("ws://127.0.0.1:9222/devtools/page/B2".into()),
            },
            selector: TargetSelector {
                title: Some("cu-smoke-B".into()),
                ..TargetSelector::default()
            },
        }
    }

    /// The fixture page: <h1>Hello B</h1><input id=q><button id=go>Go</button><p id=out>idle</p>
    fn ax_tree() -> Value {
        json!({ "nodes": [
            { "nodeId": "1", "role": { "value": "RootWebArea" }, "name": { "value": "cu-smoke-B" }, "childIds": ["2", "4", "5", "7"], "backendDOMNodeId": 1 },
            { "nodeId": "2", "parentId": "1", "role": { "value": "heading" }, "name": { "value": "Hello B" }, "childIds": ["3"], "backendDOMNodeId": 2 },
            { "nodeId": "3", "parentId": "2", "role": { "value": "StaticText" }, "name": { "value": "Hello B" }, "childIds": [], "backendDOMNodeId": 3 },
            { "nodeId": "4", "parentId": "1", "role": { "value": "textbox" }, "name": { "value": "" }, "value": { "value": "" }, "childIds": [], "backendDOMNodeId": 4,
              "properties": [{ "name": "editable", "value": { "value": "plaintext" } }] },
            { "nodeId": "5", "parentId": "1", "role": { "value": "button" }, "name": { "value": "Go" }, "childIds": ["6"], "backendDOMNodeId": 5 },
            { "nodeId": "6", "parentId": "5", "role": { "value": "StaticText" }, "name": { "value": "Go" }, "childIds": [], "backendDOMNodeId": 6 },
            { "nodeId": "7", "parentId": "1", "role": { "value": "paragraph" }, "name": { "value": "" }, "childIds": ["8"], "backendDOMNodeId": 7 },
            { "nodeId": "8", "parentId": "7", "role": { "value": "StaticText" }, "name": { "value": "idle" }, "childIds": [], "backendDOMNodeId": 8 }
        ] })
    }

    fn described(backend: u64, value: &str, text: &str) -> Value {
        let (tag, editable) = match backend {
            4 => ("input", true),
            5 => ("button", false),
            8 => ("p", false),
            _ => ("h1", false),
        };
        json!({
            "path": format!("body > {tag}"),
            "tag": tag,
            "text": text,
            "role": null,
            "name": null,
            "value": if backend == 4 { Value::from(value) } else { Value::Null },
            "checked": null,
            "attrs": if backend == 4 { "id=q" } else { "" },
            "editable": editable,
            "text_node": false
        })
    }

    fn box_model(x: f64, y: f64) -> Value {
        json!({ "model": { "content": [x, y, x + 40.0, y, x + 40.0, y + 20.0, x, y + 20.0], "width": 40, "height": 20 } })
    }

    /// A fake page: the input value and the paragraph text are state the
    /// scripted browser mutates on insertText / click, so verification is
    /// judged on a real read-back, not on the dispatch.
    fn fake_page(click_changes_dom: bool) -> Session<fake::FakeTransport> {
        let state = std::rc::Rc::new(std::cell::RefCell::new((String::new(), "idle".to_owned())));
        let shared = state.clone();
        fake::session(move |method, params| {
            let mut st = shared.borrow_mut();
            match method {
                "Accessibility.getFullAXTree" => Ok(ax_tree()),
                "DOM.getDocument" => Ok(json!({ "root": { "nodeId": 1 } })),
                "DOM.querySelectorAll" => Ok(match params["selector"].as_str() {
                    Some("#q") => json!({ "nodeIds": [104] }),
                    Some("#go") => json!({ "nodeIds": [105] }),
                    Some("p,button,input") => json!({ "nodeIds": [104, 105, 108] }),
                    Some("#none") => json!({ "nodeIds": [] }),
                    _ => return Err("DOM Error while querying".into()),
                }),
                "DOM.describeNode" => Ok(
                    json!({ "node": { "backendNodeId": params["nodeId"].as_u64().unwrap() - 100 } }),
                ),
                "DOM.resolveNode" => Ok(
                    json!({ "object": { "objectId": format!("obj{}", params["backendNodeId"]) } }),
                ),
                "Runtime.callFunctionOn" => {
                    let backend: u64 = params["objectId"].as_str().unwrap()[3..].parse().unwrap();
                    let decl = params["functionDeclaration"].as_str().unwrap();
                    let value = if decl.contains("path:") {
                        let text = if backend == 8 {
                            st.1.clone()
                        } else if backend == 5 {
                            "Go".into()
                        } else {
                            "Hello B".into()
                        };
                        described(backend, &st.0, &text)
                    } else if decl.contains("this.select") {
                        st.0.clear();
                        json!("select")
                    } else {
                        json!(if backend == 4 {
                            st.0.clone()
                        } else {
                            st.1.clone()
                        })
                    };
                    Ok(json!({ "result": { "value": value } }))
                }
                "DOM.getBoxModel" => Ok(box_model(
                    10.0,
                    20.0 * params["backendNodeId"].as_f64().unwrap(),
                )),
                "DOM.scrollIntoViewIfNeeded"
                | "DOM.focus"
                | "Emulation.setFocusEmulationEnabled"
                | "Page.enable" => Ok(json!({})),
                "Runtime.evaluate" => Ok(json!({ "result": { "value": {
                    "url": "data:text/html,B", "title": "cu-smoke-B", "ready": "complete",
                    "text_len": 7 + st.0.len() + st.1.len(), "html_len": 100, "active": null, "has_focus": false
                } } })),
                "Input.dispatchMouseEvent" => {
                    if click_changes_dom && params["type"] == "mouseReleased" {
                        st.1 = format!("clicked:{}", st.0);
                    }
                    Ok(json!({}))
                }
                "Input.insertText" => {
                    st.0.push_str(params["text"].as_str().unwrap());
                    Ok(json!({}))
                }
                "Input.dispatchKeyEvent" => Ok(json!({})),
                "Page.navigate" => Ok(json!({ "frameId": "F", "loaderId": "L" })),
                "Page.captureScreenshot" => Ok(json!({ "data": "iVBORw0KGgo=" })),
                other => Err(format!("'{other}' wasn't found")),
            }
        })
    }

    #[test]
    fn text_shapes_ax_rows_with_the_cdp_backend_and_node_ids() {
        let mut session = fake_page(false);
        let reply = text(&mut session, &ctx(), 16 * 1024).expect("text");
        assert_eq!(reply["backend"], "cdp");
        assert_eq!(reply["focus_changed"], false);
        assert_eq!(reply["target"]["id"], "B2");
        assert_eq!(reply["via"], "Accessibility.getFullAXTree");
        let rows = reply["rows"].as_array().expect("rows");
        let texts: Vec<&str> = rows.iter().map(|r| r["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["Hello B", "Go", "idle"]);
        assert_eq!(rows[1]["role"], "button");
        assert_eq!(rows[1]["id"], "5");
        assert_eq!(rows[1]["node"], 5);
        assert_eq!(reply["merged"], 2);
        assert_eq!(session.calls_made(), 1, "one round trip");
    }

    #[test]
    fn text_falls_back_to_a_dom_walk_when_accessibility_is_unavailable() {
        let mut session = fake::session(|method, params| match method {
            "Accessibility.getFullAXTree" => {
                Err("'Accessibility.getFullAXTree' wasn't found".into())
            }
            "Runtime.evaluate" => {
                assert!(params["expression"].as_str().unwrap().contains("innerText"));
                Ok(json!({ "result": { "value": [
                    { "role": "h1", "text": "Hello B", "name": null, "editable": false },
                    { "role": "input", "text": "", "name": "Search", "editable": true }
                ] } }))
            }
            other => Err(format!("unexpected {other}")),
        });
        let reply = text(&mut session, &ctx(), 16 * 1024).expect("fallback");
        assert_eq!(reply["backend"], "cdp");
        assert_eq!(reply["via"], "Runtime.evaluate innerText walk");
        assert_eq!(
            reply["fallback"]["reason"],
            "Accessibility.getFullAXTree unavailable on this target"
        );
        assert_eq!(reply["rows"][0]["id"], "dom:0");
        assert_eq!(reply["rows"][0]["role"], "h1");
        assert_eq!(reply["rows"][1]["name"], "Search");
        assert_eq!(reply["rows"][1]["editable"], true);
        // A fallback that fails too is typed from the evaluate, not swallowed.
        let mut broken = fake::session(|_, _| Err("boom".into()));
        let err = text(&mut broken, &ctx(), 100).expect_err("typed");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("boom"));
        // A transport failure of the AX call is not downgraded to the DOM walk.
        let mut silent = Session::new(fake::FakeTransport::new(|_, _, _| Vec::new()));
        silent.call_timeout = std::time::Duration::from_millis(1);
        assert_eq!(
            text(&mut silent, &ctx(), 100).expect_err("timeout").code,
            "cdp_timeout"
        );
    }

    #[test]
    fn find_by_text_lifts_to_the_button_and_describes_path_and_box() {
        let mut session = fake_page(false);
        let reply = find(&mut session, &ctx(), &NodeQuery::Text("go".into())).expect("find");
        assert_eq!(reply["total"], 1);
        assert_eq!(reply["returned"], 1);
        let node = &reply["nodes"][0];
        assert_eq!(node["node"], 5);
        assert_eq!(node["role"], "button");
        assert_eq!(node["name"], "Go");
        assert_eq!(node["path"], "body > button");
        assert_eq!(node["box"]["y"], 100.0);
        assert_eq!(node["ax_id"], "5");
        assert_eq!(reply["focus_changed"], false);
        let by_css = find(&mut session, &ctx(), &NodeQuery::Css("#q".into())).expect("css");
        assert_eq!(by_css["nodes"][0]["node"], 4);
        assert_eq!(by_css["nodes"][0]["editable"], true);
        assert_eq!(by_css["via"], "DOM.querySelectorAll");
        let by_role = find(
            &mut session,
            &ctx(),
            &NodeQuery::Role {
                role: "StaticText".into(),
                name: Some("idle".into()),
            },
        )
        .expect("role");
        assert_eq!(by_role["nodes"][0]["node"], 8);
        let missing =
            find(&mut session, &ctx(), &NodeQuery::Css("#none".into())).expect_err("none");
        assert_eq!(missing.code, "cdp_node_not_found");
        assert_eq!(missing.detail["query"]["selector"], "#none");
        let bad = find(&mut session, &ctx(), &NodeQuery::Css("[[".into())).expect_err("bad css");
        assert_eq!(bad.code, "invalid_input");
    }

    #[test]
    fn resolve_one_is_typed_on_ambiguity_with_candidates() {
        let mut session = fake_page(false);
        let err =
            resolve_one(&mut session, &NodeQuery::Css("p,button,input".into())).expect_err("three");
        assert_eq!(err.code, "cdp_node_ambiguous");
        assert_eq!(err.detail["count"], 3);
        assert_eq!(err.detail["candidates"].as_array().map(Vec::len), Some(3));
        assert_eq!(err.detail["candidates"][0]["node"], 4);
        let one = resolve_one(&mut session, &NodeQuery::Node(5)).expect("by id");
        assert_eq!(one.node, 5);
        assert_eq!(one.described["tag"], "button");
    }

    #[test]
    fn click_plans_the_box_centre_and_verifies_on_a_dom_change() {
        let mut session = fake_page(true);
        let plan =
            plan_click(&mut session, &NodeQuery::Text("Go".into()), "left", 1).expect("plan");
        assert_eq!(plan.center, (30.0, 110.0));
        assert!(plan.scrolled);
        assert_eq!(plan.json()["node"]["node"], 5);
        let methods_before = session.transport.methods().len();
        let outcome = perform_click(&mut session, &ctx(), &plan).expect("click");
        assert!(outcome.performed);
        assert!(outcome.verified, "{}", outcome.payload);
        let methods = &session.transport.methods()[methods_before..];
        assert_eq!(
            methods,
            [
                "Emulation.setFocusEmulationEnabled",
                "Input.dispatchMouseEvent",
                "Input.dispatchMouseEvent",
                "Input.dispatchMouseEvent",
                "Runtime.evaluate",
                "DOM.resolveNode",
                "Runtime.callFunctionOn",
                "Emulation.setFocusEmulationEnabled",
            ]
        );
        let sent = &session.transport.sent;
        let pressed = sent
            .iter()
            .find(|m| m["params"]["type"] == "mousePressed")
            .unwrap();
        assert_eq!(pressed["params"]["x"], 30.0);
        assert_eq!(pressed["params"]["button"], "left");
        assert_eq!(pressed["params"]["clickCount"], 1);
        assert_eq!(pressed["params"]["buttons"], 1);
        let last = sent.last().unwrap();
        assert_eq!(
            last["params"]["enabled"], false,
            "focus emulation is switched off"
        );
        assert_eq!(outcome.payload["focus_changed"], false);
        assert_eq!(
            outcome.payload["verification"]["changed"],
            json!(["text_len"])
        );
        assert!(outcome.payload["verification"]["reason"].is_null());
        // No activation method anywhere.
        assert!(
            sent.iter()
                .all(|m| m["method"] != "Target.activateTarget"
                    && m["method"] != "Page.bringToFront")
        );
    }

    #[test]
    fn click_without_an_observable_change_is_performed_but_unverified() {
        let mut session = fake_page(false);
        let plan = plan_click(&mut session, &NodeQuery::Node(8), "right", 2).expect("plan");
        let outcome = perform_click(&mut session, &ctx(), &plan).expect("click");
        assert!(outcome.performed);
        assert!(!outcome.verified);
        assert_eq!(
            outcome.payload["verification"]["reason"],
            "no_observable_change"
        );
        assert_eq!(outcome.payload["events_dispatched"], 5);
        let counts: Vec<u64> = session
            .transport
            .sent
            .iter()
            .filter(|m| m["params"]["type"] == "mousePressed")
            .map(|m| m["params"]["clickCount"].as_u64().unwrap())
            .collect();
        assert_eq!(counts, [1, 2]);
        assert_eq!(
            plan_click(&mut session, &NodeQuery::Node(8), "back", 1)
                .expect_err("button")
                .code,
            "invalid_input"
        );
        assert_eq!(
            plan_click(&mut session, &NodeQuery::Node(8), "left", 0)
                .expect_err("clicks")
                .code,
            "invalid_input"
        );
    }

    #[test]
    fn hover_reads_the_hit_target_back_without_activating_the_page() {
        let mut moved = false;
        let mut session = fake::session(move |method, params| match method {
            "Runtime.evaluate" => {
                let expression = params["expression"].as_str().unwrap();
                if expression.contains("addEventListener('mousemove'") {
                    return Ok(json!({ "result": { "value": true } }));
                }
                if expression.contains("target_matches_hit") {
                    return Ok(json!({ "result": { "value": {
                        "count": 1, "target_matches_hit": true, "x": 25, "y": 40
                    } } }));
                }
                assert!(expression.contains("elementFromPoint(25, 40)"));
                Ok(json!({ "result": { "value": {
                    "hit": "html > body > button#go",
                    "hovered": if moved { Some("html > body > button#go") } else { None },
                    "scroll": { "selector": "html", "left": 0, "top": 0,
                        "width": 800, "height": 1200, "client_width": 800, "client_height": 600 }
                } } }))
            }
            "Input.dispatchMouseEvent" => {
                assert_eq!(params["type"], "mouseMoved");
                moved = true;
                Ok(json!({}))
            }
            other => Err(format!("unexpected {other}")),
        });
        let plan = plan_point(&mut session, 25.0, 40.0).expect("plan");
        let outcome = perform_hover(&mut session, &ctx(), &plan).expect("hover");
        assert!(outcome.performed);
        assert!(outcome.verified, "{}", outcome.payload);
        assert_eq!(
            outcome.payload["verification"]["expected"]["target"],
            "html > body > button#go"
        );
        assert_eq!(outcome.payload["focus_changed"], false);
        assert!(session.transport.sent.iter().all(|call| {
            call["method"] != "Page.bringToFront" && call["method"] != "Target.activateTarget"
        }));
    }

    #[test]
    fn scroll_reads_the_planned_container_and_reports_a_boundary_honestly() {
        let mut wheel = false;
        let mut session = fake::session(move |method, params| match method {
            "Runtime.evaluate" => {
                let expression = params["expression"].as_str().unwrap();
                if expression.contains("addEventListener('scroll'") {
                    return Ok(json!({ "result": { "value": true } }));
                }
                if expression.contains("const el = document.querySelector(") {
                    Ok(json!({ "result": { "value": {
                        "selector": "html", "left": 0, "top": if wheel { 120 } else { 0 },
                        "width": 800, "height": 1200, "client_width": 800, "client_height": 600
                    } } }))
                } else {
                    Ok(json!({ "result": { "value": {
                        "hit": "html > body > main", "hovered": null,
                        "scroll": { "selector": "html", "left": 0, "top": if wheel { 120 } else { 0 },
                            "width": 800, "height": 1200, "client_width": 800, "client_height": 600 }
                    } } }))
                }
            }
            "Input.dispatchMouseEvent" => {
                assert_eq!(params["type"], "mouseWheel");
                assert_eq!(params["deltaY"], 120.0);
                wheel = true;
                Ok(json!({}))
            }
            other => Err(format!("unexpected {other}")),
        });
        let plan = plan_point(&mut session, 10.0, 10.0).expect("plan");
        let outcome = perform_scroll(&mut session, &ctx(), &plan, 0.0, 120.0).expect("scroll");
        assert!(outcome.performed);
        assert!(outcome.verified, "{}", outcome.payload);
        assert_eq!(outcome.payload["verification"]["changed"], json!(["top"]));

        let mut still = fake::session(|method, params| match method {
            "Runtime.evaluate"
                if params["expression"]
                    .as_str()
                    .unwrap()
                    .contains("addEventListener('scroll'") =>
            {
                Ok(json!({ "result": { "value": true } }))
            }
            "Runtime.evaluate"
                if params["expression"]
                    .as_str()
                    .unwrap()
                    .contains("const el = document.querySelector(") =>
            {
                Ok(json!({ "result": { "value": { "selector": "html", "left": 0, "top": 600 } } }))
            }
            "Runtime.evaluate" => Ok(json!({ "result": { "value": {
                "hit": "html > body", "hovered": null,
                "scroll": { "selector": "html", "left": 0, "top": 600 }
            } } })),
            "Input.dispatchMouseEvent" => Ok(json!({})),
            other => Err(format!("unexpected {other}")),
        });
        let plan = plan_point(&mut still, 1.0, 1.0).expect("plan");
        let outcome = perform_scroll(&mut still, &ctx(), &plan, 0.0, 120.0).expect("edge");
        assert!(outcome.performed);
        assert!(!outcome.verified);
        assert_eq!(
            outcome.payload["verification"]["reason"],
            "no_observable_scroll_change"
        );
    }

    #[test]
    fn point_actuators_reject_invalid_numbers_before_dispatch() {
        assert!(validate_pointer_coordinate("--x", f64::NAN).is_err());
        assert!(validate_pointer_coordinate("--x", -1.0).is_err());
        assert!(validate_scroll_delta("--dy", f64::INFINITY).is_err());
        let mut session = fake::session(|method, _| Err(format!("unexpected {method}")));
        assert_eq!(
            plan_point(&mut session, -1.0, 0.0)
                .expect_err("negative")
                .code,
            "invalid_input"
        );
        assert_eq!(session.calls_made(), 0);
    }

    #[test]
    fn click_on_a_node_without_a_box_is_refused_before_any_dispatch() {
        let mut session = fake::session(|method, _| match method {
            "DOM.resolveNode" => Ok(json!({ "object": { "objectId": "o" } })),
            "Runtime.callFunctionOn" => {
                Ok(json!({ "result": { "value": { "tag": "div", "editable": false } } }))
            }
            "DOM.getBoxModel" => Err("Could not compute box model.".into()),
            "DOM.scrollIntoViewIfNeeded" => Err("Node does not have a layout object".into()),
            other => Err(format!("unexpected {other}")),
        });
        let err = plan_click(&mut session, &NodeQuery::Node(9), "left", 1).expect_err("no box");
        assert_eq!(err.code, "cdp_node_not_visible");
        assert_eq!(err.detail["effect"], "not_performed");
        assert!(
            session
                .transport
                .methods()
                .iter()
                .all(|m| m != "Input.dispatchMouseEvent")
        );
    }

    #[test]
    fn fill_clears_inserts_reads_back_and_submits() {
        let mut session = fake_page(false);
        let plan = plan_fill(
            &mut session,
            &NodeQuery::Css("#q".into()),
            "hello",
            true,
            true,
        )
        .expect("plan");
        assert_eq!(plan.before_value, "");
        assert_eq!(plan.expected().as_deref(), Some("hello"));
        let outcome = perform_fill(&mut session, &ctx(), &plan).expect("fill");
        assert!(outcome.performed);
        assert!(outcome.verified, "{}", outcome.payload);
        assert_eq!(outcome.payload["verification"]["after_value"], "hello");
        assert_eq!(outcome.payload["submitted"]["dispatched"], true);
        let sent = &session.transport.sent;
        let insert = sent
            .iter()
            .find(|m| m["method"] == "Input.insertText")
            .unwrap();
        assert_eq!(insert["params"]["text"], "hello");
        let keys: Vec<&Value> = sent
            .iter()
            .filter(|m| m["method"] == "Input.dispatchKeyEvent")
            .collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0]["params"]["type"], "keyDown");
        assert_eq!(keys[0]["params"]["key"], "Enter");
        assert_eq!(keys[0]["params"]["text"], "\r");
        assert_eq!(keys[1]["params"]["type"], "keyUp");
        assert!(
            sent.iter()
                .any(|m| m["method"] == "DOM.focus" && m["params"]["backendNodeId"] == 4)
        );
        // Without --clear the text is appended and judged as an insertion.
        let plan =
            plan_fill(&mut session, &NodeQuery::Node(4), " world", false, false).expect("plan");
        assert_eq!(plan.before_value, "hello");
        let outcome = perform_fill(&mut session, &ctx(), &plan).expect("fill");
        assert!(outcome.verified);
        assert_eq!(
            outcome.payload["verification"]["after_value"],
            "hello world"
        );
        assert!(outcome.payload["submitted"].is_null());
    }

    #[test]
    fn fill_refuses_non_editable_nodes_and_empty_text_before_writing() {
        let mut session = fake_page(false);
        let err = plan_fill(
            &mut session,
            &NodeQuery::Css("#go".into()),
            "x",
            false,
            false,
        )
        .expect_err("button");
        assert_eq!(err.code, "cdp_node_not_editable");
        assert_eq!(err.detail["effect"], "not_performed");
        let err = plan_fill(&mut session, &NodeQuery::Css("#q".into()), "", false, false)
            .expect_err("empty");
        assert_eq!(err.code, "invalid_input");
        assert!(plan_fill(&mut session, &NodeQuery::Css("#q".into()), "", true, false).is_ok());
        assert!(
            session
                .transport
                .methods()
                .iter()
                .all(|m| m != "Input.insertText")
        );
        assert!(fill_verified("", "a", true, "a"));
        assert!(!fill_verified("", "a", true, "ab"));
        assert!(fill_verified("ab", "c", false, "acb"));
        assert!(!fill_verified("ab", "c", false, "ab"));
    }

    #[test]
    fn fill_mismatch_is_performed_but_unverified() {
        // A page whose input handler rewrites the value.
        let mut session = fake::session(|method, params| match method {
            "DOM.resolveNode" => Ok(json!({ "object": { "objectId": "o" } })),
            "Runtime.callFunctionOn" => Ok(json!({ "result": { "value":
                if params["functionDeclaration"].as_str().unwrap().contains("path:") {
                    json!({ "tag": "input", "editable": true, "value": "UPPER", "path": "input#q" })
                } else {
                    json!("UPPER")
                }
            } })),
            "DOM.getBoxModel" => Ok(box_model(0.0, 0.0)),
            "Runtime.evaluate" => Ok(json!({ "result": { "value": { "url": "u" } } })),
            _ => Ok(json!({})),
        });
        let plan =
            plan_fill(&mut session, &NodeQuery::Node(4), "upper", true, false).expect("plan");
        let outcome = perform_fill(&mut session, &ctx(), &plan).expect("fill");
        assert!(outcome.performed);
        assert!(!outcome.verified);
        assert_eq!(outcome.payload["verification"]["reason"], "value_mismatch");
        assert_eq!(outcome.payload["verification"]["after_value"], "UPPER");
    }

    #[test]
    fn nav_waits_for_the_load_event_and_reports_the_final_document() {
        let mut session = Session::new(fake::FakeTransport::new(
            |id, method, params| match method {
                "Page.enable" => vec![fake::result(id, json!({}))],
                "Runtime.evaluate" => vec![fake::result(
                    id,
                    json!({ "result": { "value": {
                "url": if id < 3 { "data:text/html,B" } else { "data:text/html,C" },
                "title": "after", "ready": "complete"
            } } }),
                )],
                "Page.navigate" => {
                    assert_eq!(params["url"], "data:text/html,C");
                    vec![
                        fake::result(id, json!({ "frameId": "F", "loaderId": "L" })),
                        fake::event("Page.frameNavigated", json!({})),
                        fake::event("Page.loadEventFired", json!({ "timestamp": 2.0 })),
                    ]
                }
                _ => vec![fake::error(id, -32601, "nope")],
            },
        ));
        let plan = plan_nav(&mut session, "data:text/html,C", Some(500)).expect("plan");
        assert_eq!(plan.wait_ms, 500);
        let outcome = perform_nav(&mut session, &ctx(), &plan).expect("nav");
        assert!(outcome.verified);
        assert_eq!(outcome.payload["verification"]["load_event_fired"], true);
        assert_eq!(outcome.payload["final_url"], "data:text/html,C");
        assert_eq!(outcome.payload["focus_changed"], false);
        assert_eq!(
            session.transport.methods(),
            [
                "Page.enable",
                "Runtime.evaluate",
                "Page.navigate",
                "Runtime.evaluate"
            ]
        );
        assert_eq!(
            plan_nav(&mut session, "", None).expect_err("empty").code,
            "invalid_input"
        );
        assert_eq!(
            plan_nav(&mut session, "no-scheme", None)
                .expect_err("scheme")
                .code,
            "invalid_input"
        );
        assert_eq!(
            plan_nav(&mut session, "https://x", Some(MAX_NAV_WAIT_MS + 1))
                .expect_err("wait")
                .code,
            "invalid_input"
        );
    }

    #[test]
    fn nav_error_text_is_typed_and_a_missing_load_event_is_unverified() {
        let mut session = fake::session(|method, _| match method {
            "Page.navigate" => {
                Ok(json!({ "frameId": "F", "errorText": "net::ERR_NAME_NOT_RESOLVED" }))
            }
            "Runtime.evaluate" => {
                Ok(json!({ "result": { "value": { "url": "u", "ready": "loading" } } }))
            }
            _ => Ok(json!({})),
        });
        let plan = plan_nav(&mut session, "https://nowhere.invalid/", Some(1)).expect("plan");
        let err = perform_nav(&mut session, &ctx(), &plan).expect_err("dns");
        assert_eq!(err.code, "cdp_navigation_failed");
        assert_eq!(err.detail["errorText"], "net::ERR_NAME_NOT_RESOLVED");
        let mut slow = fake::session(|method, _| match method {
            "Page.navigate" => Ok(json!({ "frameId": "F" })),
            "Runtime.evaluate" => {
                Ok(json!({ "result": { "value": { "url": "u", "ready": "loading" } } }))
            }
            _ => Ok(json!({})),
        });
        let plan = plan_nav(&mut slow, "https://slow.example/", Some(1)).expect("plan");
        let outcome = perform_nav(&mut slow, &ctx(), &plan).expect("nav");
        assert!(outcome.performed);
        assert!(!outcome.verified);
        assert_eq!(outcome.payload["verification"]["reason"], "load_timeout");
    }

    #[test]
    fn screenshot_decodes_png_and_types_a_refusal_without_activating() {
        let mut session = fake_page(false);
        let (bytes, meta) = screenshot(&mut session, &ctx(), false).expect("png");
        assert_eq!(&bytes[..4], b"\x89PNG");
        assert_eq!(meta["bytes"], 8);
        assert_eq!(meta["focus_changed"], false);
        assert_eq!(meta["activated"], false);
        assert!(
            session
                .transport
                .methods()
                .iter()
                .all(|m| m != "Page.bringToFront")
        );
        let mut refused = fake::session(|method, _| match method {
            "Page.captureScreenshot" => Err("Unable to capture screenshot".into()),
            _ => Ok(json!({})),
        });
        let err = screenshot(&mut refused, &ctx(), false).expect_err("refused");
        assert_eq!(err.code, "cdp_screenshot_unavailable");
        assert!(err.message.contains("--activate"));
        assert_eq!(err.detail["activate"], false);
        // --activate is the one explicit path that brings the tab forward.
        let mut front = fake::session(|method, _| match method {
            "Page.bringToFront" => Ok(json!({})),
            "Page.captureScreenshot" => Ok(json!({ "data": "iVBORw0KGgo=" })),
            other => Err(format!("unexpected {other}")),
        });
        let (_, meta) = screenshot(&mut front, &ctx(), true).expect("activated");
        assert_eq!(meta["focus_changed"], true);
        assert_eq!(
            front.transport.methods(),
            ["Page.bringToFront", "Page.captureScreenshot"]
        );
    }

    #[test]
    fn base64_decoder_handles_padding_and_rejects_garbage() {
        assert_eq!(base64_decode("aGk=").unwrap(), b"hi");
        assert_eq!(base64_decode("aGk").unwrap(), b"hi");
        assert_eq!(base64_decode("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(base64_decode("").unwrap(), b"");
        assert!(base64_decode("a!b").is_err());
        assert_eq!(
            NodeBox::from_model(&json!({ "model": { "content": [1, 2, 3] } })),
            None
        );
        let b = NodeBox::from_model(&box_model(10.0, 20.0)).unwrap();
        assert_eq!((b.x, b.y, b.width, b.height), (10.0, 20.0, 40.0, 20.0));
        assert_eq!(b.center(), (30.0, 30.0));
    }
}
