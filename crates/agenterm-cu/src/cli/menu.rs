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

pub fn parse_app_invoke(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let app = flag_text(args, "--app")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "app-menu-invoke requires --app <exact-name>".to_owned())?;
    let path = match flag_text(args, "--path")? {
        Some(raw) => agenterm_cu::observe::parse_menu_path(&raw)?,
        None => {
            return Err(
                "app-menu-invoke requires --path 'Menu/Item' (or a JSON array of titles)".into(),
            );
        }
    };
    if !args.is_empty() {
        return Err(format!(
            "app-menu-invoke accepts only --app EXACT --path PATH; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::AppMenuInvoke { target, app, path })
}

pub fn parse_inspect(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    parse_action(target, args, "inspect")
}

pub fn parse_invoke(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    parse_action(target, args, "invoke")
}

pub fn parse(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let Some(sub) = args.first().cloned() else {
        return Err("menu requires a subcommand: inspect | invoke".into());
    };
    args.remove(0);
    parse_action(target, args, &sub)
}

fn parse_action(target: TargetRef, args: &mut Vec<String>, sub: &str) -> Result<Command, String> {
    let Some(window) = flag_window(args)? else {
        return Err(format!("menu {sub} requires --window <handle>"));
    };
    match sub {
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
mod hyphen_alias_tests {
    use super::*;

    #[test]
    fn hyphen_inspect_parses_like_space_form() {
        let mut args = vec![
            "--window".into(),
            "27262979".into(),
            "--depth".into(),
            "3".into(),
        ];
        assert!(matches!(
            parse_inspect(TargetRef::Current, &mut args).unwrap(),
            Command::MenuInspect {
                window: 27262979,
                depth: Some(3),
                ..
            }
        ));
    }

    #[test]
    fn hyphen_invoke_parses_like_space_form() {
        let mut args = vec![
            "--window".into(),
            "7".into(),
            "--path".into(),
            "File/Quit".into(),
        ];
        assert!(matches!(
            parse_invoke(TargetRef::Current, &mut args).unwrap(),
            Command::MenuInvoke {
                window: 7,
                path,
                ..
            } if path == ["File", "Quit"]
        ));
    }

    #[test]
    fn space_subcommand_parses_like_hyphen_entrypoints() {
        let mut inspect_args = vec![
            "inspect".into(),
            "--window".into(),
            "42".into(),
            "--depth".into(),
            "2".into(),
        ];
        assert!(matches!(
            parse(TargetRef::Current, &mut inspect_args).unwrap(),
            Command::MenuInspect {
                window: 42,
                depth: Some(2),
                ..
            }
        ));
        let mut invoke_args = vec![
            "invoke".into(),
            "--window".into(),
            "42".into(),
            "--path".into(),
            "Help/About".into(),
        ];
        assert!(matches!(
            parse(TargetRef::Current, &mut invoke_args).unwrap(),
            Command::MenuInvoke {
                window: 42,
                path,
                ..
            } if path == ["Help", "About"]
        ));
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

    #[test]
    fn app_global_invocation_has_one_exact_path_shape() {
        let mut args = vec![
            "--app".into(),
            "Editor".into(),
            "--path".into(),
            "File/Save".into(),
        ];
        assert!(matches!(
            parse_app_invoke(TargetRef::Current, &mut args).unwrap(),
            Command::AppMenuInvoke { app, path, .. }
                if app == "Editor" && path == ["File", "Save"]
        ));
        assert!(parse_app_invoke(TargetRef::Current, &mut Vec::new()).is_err());
    }
}
