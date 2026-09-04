//! Browser page & tabs: the CDP verbs (`page-js`, `page-targets`, and the
//! background-tab verbs `page find|click|download|hover|scroll|drag|dialog|files|fill|type|nav|screenshot`), the page
//! reader (`page-text`: a11y with `--window`, CDP with a target selector),
//! the tab strip (`tab list|select|close`), the profile verbs (`browser
//! profiles|open`), and the MCU `page` group word.

use agenterm_cu::{Command, TargetRef};

use super::verbs::VerbSpec;
use super::{flag_parsed, flag_text, flag_window, take_switch};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spelled {
        "page" => page_group(target, args),
        "tab" => tab(target, None, args),
        "browser" => browser(target, None, args),
        _ => match spec.name {
            "page-js" => page_js(target, args),
            "page-targets" => page_targets(target, args),
            "page-text" => page_text(target, args),
            "page-find" => page_find(target, args),
            "page-click" => page_click(target, args),
            "page-download" => page_download(target, args),
            "page-hover" => page_hover(target, args),
            "page-scroll" => page_scroll(target, args),
            "page-drag" => page_drag(target, args),
            "page-dialog" => page_dialog(target, args),
            "page-files" => page_files(target, args),
            "page-fill" => page_fill(target, args),
            "page-type" => page_type(target, args),
            "page-nav" => page_nav(target, args),
            "page-screenshot" => page_screenshot(target, args),
            "tab-list" => tab(target, Some("list"), args),
            "tab-select" => tab(target, Some("select"), args),
            "tab-close" => tab(target, Some("close"), args),
            "browser-profiles" => browser(target, Some("profiles"), args),
            "browser-open" => browser(target, Some("open"), args),
            other => Err(format!("unknown command '{other}'")),
        },
    }
}

/// MCU `page`: `page read --js` -> page-js, `page read` (no --js) -> the
/// CDP `page text`, `page targets` -> page-targets, `page text` ->
/// page-text (a11y with --window, CDP with a target selector), and the
/// CDP background-tab verbs `page find | click | fill | nav | screenshot`;
/// anything else stays typed (`Command::Align`).
fn page_group(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let sub = args.first().cloned();
    match sub.as_deref() {
        Some("targets") => {
            args.remove(0);
            return page_targets(target, args);
        }
        Some("text") => {
            args.remove(0);
            return page_text(target, args);
        }
        Some("find") => {
            args.remove(0);
            return page_find(target, args);
        }
        Some("click") => {
            args.remove(0);
            return page_click(target, args);
        }
        Some("download") => {
            args.remove(0);
            return page_download(target, args);
        }
        Some("hover") => {
            args.remove(0);
            return page_hover(target, args);
        }
        Some("scroll") => {
            args.remove(0);
            return page_scroll(target, args);
        }
        Some("drag") => {
            args.remove(0);
            return page_drag(target, args);
        }
        Some("dialog") => {
            args.remove(0);
            return page_dialog(target, args);
        }
        Some("files") => {
            args.remove(0);
            return page_files(target, args);
        }
        Some("fill") => {
            args.remove(0);
            return page_fill(target, args);
        }
        Some("type") => {
            args.remove(0);
            return page_type(target, args);
        }
        Some("nav") => {
            args.remove(0);
            return page_nav(target, args);
        }
        Some("screenshot") => {
            args.remove(0);
            return page_screenshot(target, args);
        }
        Some("read") => {
            args.remove(0);
            if let Some(index) = args.iter().position(|arg| arg == "--js") {
                args[index] = "--expression".into();
                return page_js(target, args);
            }
            // MCU `page read` without --js: the page's words over CDP.
            return page_text(target, args);
        }
        _ => {}
    }
    if let Some(index) = args.iter().position(|arg| arg == "--js") {
        args[index] = "--expression".into();
    }
    if !args.iter().any(|arg| arg == "--expression") {
        return Ok(Command::Align {
            target,
            group: "page".into(),
        });
    }
    page_js(target, args)
}

/// `(port, pid, target_id, target_url, target_title, target_match)`: the CDP target
/// selector every CDP verb takes. At most one of the four selectors,
/// none of them empty.
type CdpTargetFlags = (
    Option<u16>,
    Option<u32>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn cdp_target_flags(verb: &str, args: &mut Vec<String>) -> Result<CdpTargetFlags, String> {
    let port = flag_parsed::<u16>(args, "--port")?;
    let pid = flag_parsed::<u32>(args, "--pid")?;
    if port.is_some() && pid.is_some() {
        return Err(format!(
            "{verb} takes at most one of --port N and --pid PID"
        ));
    }
    let target_id = flag_text(args, "--target-id")?;
    let target_url = flag_text(args, "--target-url")?;
    let target_title = flag_text(args, "--target-title")?;
    let target_match = flag_text(args, "--match")?;
    let selectors = usize::from(target_id.is_some())
        + usize::from(target_url.is_some())
        + usize::from(target_title.is_some())
        + usize::from(target_match.is_some());
    if selectors > 1 {
        return Err(format!(
            "{verb} takes at most one of --target-id ID, --target-url SUB, --target-title SUB, --match SUB"
        ));
    }
    if [&target_id, &target_url, &target_title, &target_match]
        .iter()
        .any(|value| value.as_deref().is_some_and(|raw| raw.trim().is_empty()))
    {
        return Err(format!(
            "{verb} --target-id / --target-url / --target-title / --match must not be empty"
        ));
    }
    Ok((port, pid, target_id, target_url, target_title, target_match))
}

fn page_js(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let window = flag_window(args)?;
    let expression = flag_text(args, "--expression")?;
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page-js", args)?;
    if !args.is_empty() {
        return Err(format!(
            "page-js accepts only --window H --expression EXPR --port N \
             [--target-id ID | --target-url SUB | --target-title SUB | --match SUB]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageJs {
        target,
        window,
        expression,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
    })
}

fn page_text(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let window = flag_window(args)?;
    let max_bytes = flag_parsed::<usize>(args, "--max-bytes")?;
    agenterm_cu::page_text::validate_max_bytes(max_bytes)?;
    let within = match flag_text(args, "--within")? {
        Some(raw) => Some(agenterm_cu::observe::parse_within(&raw)?),
        None => None,
    };
    let depth = flag_parsed::<u32>(args, "--depth")?;
    let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
    agenterm_cu::observe::validate_budget(depth, max_nodes)?;
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page text", args)?;
    let cdp = port.is_some()
        || pid.is_some()
        || target_id.is_some()
        || target_url.is_some()
        || target_title.is_some()
        || target_match.is_some();
    if window.is_none() && !cdp {
        return Err(
            "page text needs --window HANDLE (a11y tree of the active tab) or a CDP target \
             (--target-id ID | --target-url SUB | --target-title SUB | --match SUB [--port N]; reaches background tabs)"
                .into(),
        );
    }
    if !args.is_empty() {
        return Err(format!(
            "page text accepts only --window H --max-bytes N --within X,Y,W,H --depth N --max-nodes N, \
             or --max-bytes N with --port N [--target-id ID | --target-url SUB | --target-title SUB | --match SUB]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageText {
        target,
        window,
        max_bytes,
        within,
        depth,
        max_nodes,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
    })
}

/// The node-addressing flags shared by `page find` / `click` / `fill`:
/// `--selector CSS | --text SUB | --role R [--name SUB] | --node ID`.
struct NodeFlags {
    selector: Option<String>,
    text: Option<String>,
    role: Option<String>,
    name: Option<String>,
    node: Option<u64>,
}

fn node_flags(verb: &str, args: &mut Vec<String>, allowed: &[&str]) -> Result<NodeFlags, String> {
    let selector = flag_text(args, "--selector")?;
    let text = flag_text(args, "--text")?;
    let role = flag_text(args, "--role")?;
    let name = flag_text(args, "--name")?;
    let node = flag_parsed::<u64>(args, "--node")?;
    let given: Vec<&str> = [
        ("--selector", selector.is_some()),
        ("--text", text.is_some()),
        ("--role", role.is_some()),
        ("--node", node.is_some()),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(flag, _)| flag)
    .collect();
    if let Some(stray) = given.iter().find(|flag| !allowed.contains(*flag)) {
        return Err(format!(
            "{verb} does not take {stray}; it names one node with {}",
            allowed.join(" | ")
        ));
    }
    if given.len() != 1 {
        return Err(format!(
            "{verb} names one node with exactly one of {} (got {})",
            allowed.join(" | "),
            given.len()
        ));
    }
    if name.is_some() && role.is_none() {
        return Err(format!("{verb} --name SUB only narrows --role R"));
    }
    for (flag, value) in [
        ("--selector", &selector),
        ("--text", &text),
        ("--role", &role),
        ("--name", &name),
    ] {
        if value.as_deref().is_some_and(|raw| raw.trim().is_empty()) {
            return Err(format!("{verb} {flag} must not be empty"));
        }
    }
    Ok(NodeFlags {
        selector,
        text,
        role,
        name,
        node,
    })
}

fn page_find(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page find", args)?;
    let node = node_flags("page find", args, &["--selector", "--text", "--role"])?;
    if !args.is_empty() {
        return Err(format!(
            "page find accepts only [--port N] [--target-id ID | --target-url SUB | --target-title SUB | --match SUB] \
             (--selector CSS | --text SUB | --role R [--name SUB]); unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageFind {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        selector: node.selector,
        text: node.text,
        role: node.role,
        name: node.name,
    })
}

fn page_click(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page click", args)?;
    let x = flag_parsed::<f64>(args, "--x")?;
    let y = flag_parsed::<f64>(args, "--y")?;
    let coordinates = x.is_some() || y.is_some();
    if coordinates && (x.is_none() || y.is_none()) {
        return Err("page click coordinates require both --x X and --y Y".into());
    }
    if let (Some(x), Some(y)) = (x, y) {
        agenterm_cu::cdp::page::validate_pointer_coordinate("page click --x", x)?;
        agenterm_cu::cdp::page::validate_pointer_coordinate("page click --y", y)?;
    }
    let node = if coordinates {
        if args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--selector" | "--text" | "--node"))
        {
            return Err("page click takes either --x X --y Y or one of --selector / --text / --node, not both".into());
        }
        NodeFlags {
            selector: None,
            text: None,
            role: None,
            name: None,
            node: None,
        }
    } else {
        node_flags("page click", args, &["--selector", "--text", "--node"])?
    };
    let button = flag_text(args, "--button")?;
    if let Some(button) = button.as_deref()
        && !matches!(button, "left" | "right" | "middle")
    {
        return Err(format!(
            "page click --button accepts left | right | middle, got {button:?}"
        ));
    }
    let clicks = flag_parsed::<u32>(args, "--clicks")?;
    if clicks.is_some_and(|clicks| !(1..=3).contains(&clicks)) {
        return Err("page click --clicks accepts 1..=3".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "page click accepts only [--port N] [--target-id ID | --target-url SUB | --target-title SUB | --match SUB] \
             ((--selector CSS | --text SUB | --node ID) | --x X --y Y) [--button left|right|middle] [--clicks N]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageClick {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        selector: node.selector,
        text: node.text,
        node: node.node,
        x,
        y,
        button,
        clicks,
    })
}

fn page_download(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page download", args)?;
    let node = node_flags("page download", args, &["--selector", "--text", "--node"])?;
    let Some(download_dir) = flag_text(args, "--download-dir")? else {
        return Err("page download requires --download-dir PATH".into());
    };
    let wait_ms = flag_parsed::<u64>(args, "--wait-ms")?;
    if wait_ms.is_some_and(|value| !(1..=300_000).contains(&value)) {
        return Err("page download --wait-ms accepts 1..=300000".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "page download accepts only CDP target flags, one of --selector / --text / --node, --download-dir PATH and [--wait-ms N]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageDownload {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        selector: node.selector,
        text: node.text,
        node: node.node,
        download_dir,
        wait_ms,
    })
}

fn page_hover(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page hover", args)?;
    let Some(x) = flag_parsed::<f64>(args, "--x")? else {
        return Err("page hover requires --x X --y Y".into());
    };
    let Some(y) = flag_parsed::<f64>(args, "--y")? else {
        return Err("page hover requires --x X --y Y".into());
    };
    agenterm_cu::cdp::page::validate_pointer_coordinate("page hover --x", x)?;
    agenterm_cu::cdp::page::validate_pointer_coordinate("page hover --y", y)?;
    if !args.is_empty() {
        return Err(format!(
            "page hover accepts only [--port N] [--target-id ID | --target-url SUB | --target-title SUB | --match SUB] --x X --y Y; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageHover {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        x,
        y,
    })
}

fn page_scroll(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page scroll", args)?;
    let Some(x) = flag_parsed::<f64>(args, "--x")? else {
        return Err("page scroll requires --x X --y Y [--dx DX] [--dy DY]".into());
    };
    let Some(y) = flag_parsed::<f64>(args, "--y")? else {
        return Err("page scroll requires --x X --y Y [--dx DX] [--dy DY]".into());
    };
    let dx = flag_parsed::<f64>(args, "--dx")?;
    let dy = flag_parsed::<f64>(args, "--dy")?;
    agenterm_cu::cdp::page::validate_pointer_coordinate("page scroll --x", x)?;
    agenterm_cu::cdp::page::validate_pointer_coordinate("page scroll --y", y)?;
    agenterm_cu::cdp::page::validate_scroll_delta("page scroll --dx", dx.unwrap_or(0.0))?;
    agenterm_cu::cdp::page::validate_scroll_delta("page scroll --dy", dy.unwrap_or(120.0))?;
    if dx.unwrap_or(0.0) == 0.0 && dy.unwrap_or(120.0) == 0.0 {
        return Err("page scroll requires a non-zero --dx or --dy".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "page scroll accepts only [--port N] [--target-id ID | --target-url SUB | --target-title SUB | --match SUB] --x X --y Y [--dx DX] [--dy DY]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageScroll {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        x,
        y,
        dx,
        dy,
    })
}

fn page_drag(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page drag", args)?;
    let mut coordinates = Vec::with_capacity(4);
    for flag in ["--x1", "--y1", "--x2", "--y2"] {
        let value = flag_parsed::<f64>(args, flag)?;
        coordinates.push((flag, value));
    }
    if coordinates.iter().any(|(_, value)| value.is_none()) && args.len() >= 4 {
        for item in &mut coordinates {
            if item.1.is_none() {
                item.1 = args.first().and_then(|raw| raw.parse::<f64>().ok());
                if item.1.is_some() {
                    args.remove(0);
                }
            }
        }
    }
    let [(_, Some(x1)), (_, Some(y1)), (_, Some(x2)), (_, Some(y2))] = coordinates.as_slice()
    else {
        return Err(
            "page drag requires --x1 X --y1 Y --x2 X --y2 Y (or four MCU positional coordinates)"
                .into(),
        );
    };
    for (flag, value) in [("--x1", *x1), ("--y1", *y1), ("--x2", *x2), ("--y2", *y2)] {
        agenterm_cu::cdp::page::validate_pointer_coordinate(&format!("page drag {flag}"), value)?;
    }
    if x1 == x2 && y1 == y2 {
        return Err("page drag requires distinct start and end points".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "page drag accepts only target flags plus --x1 X --y1 Y --x2 X --y2 Y; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageDrag {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        x1: *x1,
        y1: *y1,
        x2: *x2,
        y2: *y2,
    })
}

fn page_dialog(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page dialog", args)?;
    let dismiss = take_switch(args, "--dismiss");
    let text = flag_text(args, "--text")?;
    let wait_ms = flag_parsed::<u64>(args, "--wait-ms")?;
    if dismiss && text.is_some() {
        return Err("page dialog --text cannot be combined with --dismiss".into());
    }
    if text
        .as_ref()
        .is_some_and(|value| value.len() > agenterm_cu::cdp::page::MAX_FILL_BYTES)
    {
        return Err(format!(
            "page dialog --text exceeds {} UTF-8 bytes",
            agenterm_cu::cdp::page::MAX_FILL_BYTES
        ));
    }
    if wait_ms.is_some_and(|value| value == 0 || value > agenterm_cu::cdp::page::MAX_DIALOG_WAIT_MS)
    {
        return Err(format!(
            "page dialog --wait-ms must be within 1..={}",
            agenterm_cu::cdp::page::MAX_DIALOG_WAIT_MS
        ));
    }
    if !args.is_empty() {
        return Err(format!(
            "page dialog accepts only target flags plus [--dismiss] [--text TEXT] [--wait-ms N]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageDialog {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        dismiss,
        text,
        wait_ms,
    })
}

fn page_files(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page files", args)?;
    let selector = flag_text(args, "--selector")?;
    let mut node = flag_parsed::<u64>(args, "--node")?;
    if selector.is_some() && node.is_some() {
        return Err("page files accepts exactly one of --selector CSS | --node ID".into());
    }
    if selector.is_none() && node.is_none() {
        node = args.first().and_then(|value| value.parse::<u64>().ok());
        if node.is_some() {
            args.remove(0);
        }
    }
    if selector
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
        || node == Some(0)
    {
        return Err("page files requires a non-empty --selector or positive --node".into());
    }
    if selector.is_none() && node.is_none() {
        return Err(
            "page files requires --selector CSS | --node ID (MCU positional NODE is accepted)"
                .into(),
        );
    }
    let files = std::mem::take(args);
    if files.is_empty() || files.len() > agenterm_cu::cdp::page::MAX_FILES {
        return Err(format!(
            "page files requires 1..={} local files",
            agenterm_cu::cdp::page::MAX_FILES
        ));
    }
    if files.iter().any(|path| {
        path.is_empty()
            || path.len() > agenterm_cu::cdp::page::MAX_FILE_PATH_BYTES
            || path.contains('\0')
            || !std::path::Path::new(path).is_absolute()
    }) {
        return Err("page files requires bounded absolute paths on the browser host".into());
    }
    Ok(Command::PageFiles {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        selector,
        node,
        files,
    })
}

fn page_fill(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page fill", args)?;
    // `--text` is the payload here, not an addressing form.
    let Some(text) = flag_text(args, "--text")? else {
        return Err("page fill requires --text TEXT (may be empty only with --clear)".into());
    };
    let clear = take_switch(args, "--clear");
    let submit = take_switch(args, "--submit");
    if text.is_empty() && !clear {
        return Err("page fill --text is empty; pass --clear to empty the field on purpose".into());
    }
    let node = node_flags("page fill", args, &["--selector", "--node"])?;
    if !args.is_empty() {
        return Err(format!(
            "page fill accepts only [--port N] [--target-id ID | --target-url SUB | --target-title SUB | --match SUB] \
             (--selector CSS | --node ID) --text TEXT [--clear] [--submit]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageFill {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        selector: node.selector,
        node: node.node,
        text,
        clear,
        submit,
    })
}

fn page_type(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page type", args)?;
    let text = flag_text(args, "--text")?.or_else(|| {
        if args.len() == 1 {
            Some(args.remove(0))
        } else {
            None
        }
    });
    let Some(text) = text else {
        return Err("page type needs exactly TEXT or --text TEXT".into());
    };
    if text.is_empty() {
        return Err("page type TEXT must not be empty".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "page type accepts only TEXT|--text TEXT [--port N] [--target-id ID | --target-url SUB | --target-title SUB | --match SUB]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageType {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        text,
    })
}

fn page_nav(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page nav", args)?;
    let Some(url) = flag_text(args, "--url")? else {
        return Err("page nav requires --url URL".into());
    };
    agenterm_cu::cdp::page::validate_nav_url(&url)?;
    let wait_ms = flag_parsed::<u64>(args, "--wait-ms")?;
    agenterm_cu::cdp::page::validate_nav_wait(wait_ms)?;
    if !args.is_empty() {
        return Err(format!(
            "page nav accepts only [--port N] [--target-id ID | --target-url SUB | --target-title SUB | --match SUB] \
             --url URL [--wait-ms N]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageNav {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        url,
        wait_ms,
    })
}

fn page_screenshot(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let (port, pid, target_id, target_url, target_title, target_match) =
        cdp_target_flags("page screenshot", args)?;
    let Some(out) = flag_text(args, "--out")? else {
        return Err("page screenshot requires --out PATH (PNG)".into());
    };
    if out.trim().is_empty() {
        return Err("page screenshot --out must not be empty".into());
    }
    let replace = take_switch(args, "--replace");
    let activate = take_switch(args, "--activate");
    if !args.is_empty() {
        return Err(format!(
            "page screenshot accepts only [--port N] [--target-id ID | --target-url SUB | --target-title SUB | --match SUB] \
             --out PATH [--replace] [--activate]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageScreenshot {
        target,
        port,
        pid,
        target_id,
        target_url,
        target_title,
        target_match,
        out,
        replace,
        activate,
    })
}

fn page_targets(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let port = flag_parsed::<u16>(args, "--port")?;
    let pid = flag_parsed::<u32>(args, "--pid")?;
    if port.is_some() && pid.is_some() {
        return Err("page targets takes at most one of --port N and --pid PID".into());
    }
    let browser_profile = flag_text(args, "--browser-profile")?;
    if browser_profile
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err("page targets --browser-profile must not be empty".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "page targets accepts only [--port N | --pid PID] --browser-profile SUB; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageTargets {
        target,
        port,
        pid,
        browser_profile,
    })
}

/// `browser profiles` / `browser open`: the Chromium-family profile verbs.
/// The flat spellings pass their sub-command in.
fn browser(
    target: TargetRef,
    sub: Option<&str>,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    let sub = match sub {
        Some(sub) => sub.to_owned(),
        None => {
            let Some(sub) = args.first().cloned() else {
                return Err("browser requires a subcommand: profiles | open".into());
            };
            args.remove(0);
            sub
        }
    };
    match sub.as_str() {
        "profiles" => {
            let app = flag_text(args, "--app")?;
            if app.as_deref().is_some_and(|value| value.trim().is_empty()) {
                return Err("browser profiles --app must not be empty".into());
            }
            if !args.is_empty() {
                return Err(format!(
                    "browser profiles accepts only --app SUB; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::BrowserProfiles { target, app })
        }
        "open" => {
            let Some(profile) = flag_text(args, "--profile")? else {
                return Err("browser open requires --profile NAME (see `browser profiles`)".into());
            };
            if profile.trim().is_empty() {
                return Err("browser open --profile must not be empty".into());
            }
            let url = flag_text(args, "--url")?;
            if let Some(url) = url.as_deref() {
                agenterm_cu::browser_profiles::validate_url(url)?;
            }
            let app = flag_text(args, "--app")?;
            if app.as_deref().is_some_and(|value| value.trim().is_empty()) {
                return Err("browser open --app must not be empty".into());
            }
            let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?;
            if !args.is_empty() {
                return Err(format!(
                    "browser open accepts only --profile NAME [--url URL] [--app SUB] [--timeout-ms N]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::BrowserOpen {
                target,
                profile,
                url,
                app,
                timeout_ms,
            })
        }
        other => Err(format!(
            "unknown browser subcommand {other:?}; expected profiles | open"
        )),
    }
}

/// `tab list` / `tab select` / `tab close`: the tab strip through the
/// a11y tree, background only (`tab close --port` may close over CDP).
/// The flat spellings pass their sub-command in.
fn tab(target: TargetRef, sub: Option<&str>, args: &mut Vec<String>) -> Result<Command, String> {
    let sub = match sub {
        Some(sub) => sub.to_owned(),
        None => {
            let Some(sub) = args.first().cloned() else {
                return Err("tab requires a subcommand: list | select | close".into());
            };
            args.remove(0);
            sub
        }
    };
    let Some(window) = flag_window(args)? else {
        return Err(format!("tab {sub} requires --window <handle>"));
    };
    match sub.as_str() {
        "list" => {
            if !args.is_empty() {
                return Err(format!(
                    "tab list accepts only --window H; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::TabList { target, window })
        }
        "select" => {
            let title = flag_text(args, "--title")?;
            let index = flag_parsed::<usize>(args, "--index")?;
            agenterm_cu::tab_strip::TabSpec::from_parts(title.as_deref(), index)?;
            if !args.is_empty() {
                return Err(format!(
                    "tab select accepts only --window H (--title SUB | --index N); unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::TabSelect {
                target,
                window,
                title,
                index,
            })
        }
        "close" => {
            let title = flag_text(args, "--title")?;
            let index = flag_parsed::<usize>(args, "--index")?;
            let exact = take_switch(args, "--exact");
            let expect = flag_text(args, "--expect")?;
            let port = flag_parsed::<u16>(args, "--port")?;
            if title.is_some() && index.is_some() {
                return Err("tab close takes --title T --exact or --index N, not both".into());
            }
            if !args.is_empty() {
                return Err(format!(
                    "tab close accepts only --window H (--title T --exact | --index N) --expect gone [--port N]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::TabClose {
                target,
                window,
                title,
                index,
                exact,
                expect,
                port,
            })
        }
        other => Err(format!(
            "unknown tab subcommand {other:?}; expected list | select | close"
        )),
    }
}
