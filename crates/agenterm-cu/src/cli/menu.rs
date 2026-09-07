//! `menu inspect` / `menu invoke`: closed shapes, background only. Both
//! sub-commands share the group word, so one parser owns them and the two
//! accessibility families forward here.

use agenterm_cu::{Command, TargetRef};

use super::{flag_parsed, flag_text, flag_window, take_switch};

pub fn parse_app_inspect(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let app = flag_text(args, "--app")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "app-menu-inspect requires --app <exact-name>".to_owned())?;
    let depth = flag_parsed::<u32>(args, "--depth")?;
    let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
    let title = flag_text(args, "--title")?;
    let exact = take_switch(args, "--exact");
    if exact && title.is_none() {
        return Err("app-menu-inspect --exact requires --title".into());
    }
    let enabled = match flag_text(args, "--enabled")? {
        Some(raw) => match raw.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => return Err("app-menu-inspect --enabled takes true or false".into()),
        },
        None => None,
    };
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let max = flag_parsed::<usize>(args, "--max")?;
    if !args.is_empty() {
        return Err(format!(
            "app-menu-inspect accepts only --app NAME --depth N --max-nodes N --title T [--exact] \
             --enabled true|false --offset N --max N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::AppMenuInspect {
        target,
        app,
        depth,
        max_nodes,
        title,
        exact,
        enabled,
        offset,
        max,
    })
}

pub fn parse(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let Some(sub) = args.first().cloned() else {
        return Err("menu requires a subcommand: inspect | invoke".into());
    };
    args.remove(0);
    let Some(window) = flag_window(args)? else {
        return Err(format!("menu {sub} requires --window <handle>"));
    };
    match sub.as_str() {
        "inspect" => {
            let depth = flag_parsed::<u32>(args, "--depth")?;
            let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
            let title = flag_text(args, "--title")?;
            let exact = take_switch(args, "--exact");
            if exact && title.is_none() {
                return Err("menu inspect --exact requires --title".into());
            }
            let enabled = match flag_text(args, "--enabled")? {
                Some(raw) => match raw.as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => return Err("menu inspect --enabled takes true or false".into()),
                },
                None => None,
            };
            let offset = flag_parsed::<usize>(args, "--offset")?;
            let max = flag_parsed::<usize>(args, "--max")?;
            if !args.is_empty() {
                return Err(format!(
                    "menu inspect accepts only --window H --depth N --max-nodes N --title T [--exact] \
                     --enabled true|false --offset N --max N; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::MenuInspect {
                target,
                window,
                depth,
                max_nodes,
                title,
                exact,
                enabled,
                offset,
                max,
            })
        }
        "invoke" => {
            let path = match flag_text(args, "--path")? {
                Some(raw) => agenterm_cu::observe::parse_menu_path(&raw)?,
                None => {
                    return Err(
                        "menu invoke requires --path 'Menu/Item' (or a JSON array of titles)"
                            .into(),
                    );
                }
            };
            if !args.is_empty() {
                return Err(format!(
                    "menu invoke accepts only --window H --path PATH; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::MenuInvoke {
                target,
                window,
                path,
            })
        }
        other => Err(format!(
            "unknown menu subcommand {other:?}; expected inspect | invoke"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_global_inspection_has_one_exact_bounded_shape() {
        let mut args = vec![
            "--app".into(),
            "Editor".into(),
            "--depth".into(),
            "2".into(),
            "--max-nodes".into(),
            "500".into(),
            "--title".into(),
            "Open".into(),
            "--exact".into(),
        ];
        assert!(matches!(
            parse_app_inspect(TargetRef::Current, &mut args).unwrap(),
            Command::AppMenuInspect {
                app,
                depth: Some(2),
                max_nodes: Some(500),
                title: Some(title),
                exact: true,
                ..
            } if app == "Editor" && title == "Open"
        ));
        assert!(parse_app_inspect(TargetRef::Current, &mut Vec::new()).is_err());
        assert!(
            parse_app_inspect(
                TargetRef::Current,
                &mut vec!["--app".into(), "Editor".into(), "--unknown".into()]
            )
            .is_err()
        );
    }
}
