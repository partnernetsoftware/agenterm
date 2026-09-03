//! Browser page & tabs: the CDP verbs (`page-js`, `page-targets`), the a11y
//! page reader (`page-text`), the tab strip, and the MCU `page` group word.

use agenterm_cu::{Command, TargetRef};

use super::verbs::VerbSpec;
use super::{flag_parsed, flag_text, flag_window};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spelled {
        "page" => page_group(target, args),
        "tab" => tab(target, None, args),
        _ => match spec.name {
            "page-js" => page_js(target, args),
            "page-targets" => page_targets(target, args),
            "page-text" => page_text(target, args),
            "tab-list" => tab(target, Some("list"), args),
            "tab-select" => tab(target, Some("select"), args),
            other => Err(format!("unknown command '{other}'")),
        },
    }
}

/// MCU `page`: `page read --js` -> page-js, `page targets` -> page-targets,
/// `page text` -> page-text; anything else stays typed (`Command::Align`).
fn page_group(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    if args.first().map(String::as_str) == Some("targets") {
        args.remove(0);
        return page_targets(target, args);
    }
    if args.first().map(String::as_str) == Some("text") {
        args.remove(0);
        return page_text(target, args);
    }
    if args.first().map(String::as_str) == Some("read") {
        args.remove(0);
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

fn page_js(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let window = flag_window(args)?;
    let expression = flag_text(args, "--expression")?;
    let port = flag_parsed::<u16>(args, "--port")?;
    let target_id = flag_text(args, "--target-id")?;
    let target_url = flag_text(args, "--target-url")?;
    let target_title = flag_text(args, "--target-title")?;
    let selectors = usize::from(target_id.is_some())
        + usize::from(target_url.is_some())
        + usize::from(target_title.is_some());
    if selectors > 1 {
        return Err(
            "page-js takes at most one of --target-id ID, --target-url SUB, --target-title SUB"
                .into(),
        );
    }
    if [&target_id, &target_url, &target_title]
        .iter()
        .any(|value| value.as_deref().is_some_and(|raw| raw.trim().is_empty()))
    {
        return Err("page-js --target-id / --target-url / --target-title must not be empty".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "page-js accepts only --window H --expression EXPR --port N \
             [--target-id ID | --target-url SUB | --target-title SUB]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageJs {
        target,
        window,
        expression,
        port,
        target_id,
        target_url,
        target_title,
    })
}

fn page_text(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let Some(window) = flag_window(args)? else {
        return Err("page text requires --window <handle>".into());
    };
    let max_bytes = flag_parsed::<usize>(args, "--max-bytes")?;
    agenterm_cu::page_text::validate_max_bytes(max_bytes)?;
    let within = match flag_text(args, "--within")? {
        Some(raw) => Some(agenterm_cu::observe::parse_within(&raw)?),
        None => None,
    };
    let depth = flag_parsed::<u32>(args, "--depth")?;
    let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
    agenterm_cu::observe::validate_budget(depth, max_nodes)?;
    if !args.is_empty() {
        return Err(format!(
            "page text accepts only --window H --max-bytes N --within X,Y,W,H --depth N --max-nodes N; unexpected {:?}",
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
    })
}

fn page_targets(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let port = flag_parsed::<u16>(args, "--port")?;
    if !args.is_empty() {
        return Err(format!(
            "page targets accepts only --port N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageTargets { target, port })
}

/// `tab list` / `tab select`: the tab strip through the a11y tree,
/// background only. The flat spellings pass their sub-command in.
fn tab(target: TargetRef, sub: Option<&str>, args: &mut Vec<String>) -> Result<Command, String> {
    let sub = match sub {
        Some(sub) => sub.to_owned(),
        None => {
            let Some(sub) = args.first().cloned() else {
                return Err("tab requires a subcommand: list | select".into());
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
        other => Err(format!(
            "unknown tab subcommand {other:?}; expected list | select"
        )),
    }
}
