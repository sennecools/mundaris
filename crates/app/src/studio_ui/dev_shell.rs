//! astrum-dev `studio` commands: workspace, panel tab and species browser,
//! so automation (other lanes, the scorer scripts) can show and capture
//! Studio panels. Species edits regrow the line-up; nothing is saved.
use anyhow::{Result, anyhow, bail, ensure};
use astrum_app::{
    developer_protocol::DevCommand,
    developer_service::DevShell,
    studio::{
        species::{SpeciesAction, SpeciesEditor},
        view::ParamEdit,
    },
};
use astrum_core::params::{ParamKind, ParamValue};
use serde_json::Value;

use super::{Workspace, dock::Tab};

/// Layout changes asked for by automation; the next UI frame applies them.
#[derive(Debug, Default)]
pub struct ShellRequests {
    pub workspace: Option<Workspace>,
    pub tab: Option<Tab>,
}

/// The Studio state a `studio` command may change.
pub struct Shell<'a> {
    pub species: &'a mut SpeciesEditor,
    pub requests: &'a mut ShellRequests,
}

impl DevShell for Shell<'_> {
    fn studio(&mut self, command: &DevCommand) -> Result<()> {
        let DevCommand::Studio {
            workspace,
            tab,
            species,
            species_revert,
            species_params,
            variants,
        } = command
        else {
            bail!("not a studio command");
        };
        // Resolve every name before changing anything.
        let workspace = workspace.as_deref().map(workspace_named).transpose()?;
        let tab = tab.as_deref().map(tab_named).transpose()?;
        let species = species
            .as_deref()
            .map(|stem| {
                self.species
                    .view()
                    .species
                    .iter()
                    .position(|(name, _)| name == stem)
                    .ok_or_else(|| anyhow!("unknown species {stem:?}"))
            })
            .transpose()?;
        if let Some(count) = variants {
            ensure!(*count >= 1, "variants must be at least 1");
        }

        if workspace.is_some() {
            self.requests.workspace = workspace;
        }
        if tab.is_some() {
            self.requests.tab = tab;
        }
        if let Some(index) = species {
            self.species.action(SpeciesAction::Select(index));
        }
        if *species_revert {
            self.species.action(SpeciesAction::Revert);
        }
        for (key, value) in species_params {
            self.species_param(key, value)?;
        }
        if let Some(count) = variants {
            self.species.action(SpeciesAction::Variants(*count));
        }
        Ok(())
    }
}

impl Shell<'_> {
    /// Sets one genome or niche parameter of the selected species by key.
    fn species_param(&mut self, key: &str, value: &Value) -> Result<()> {
        let view = self.species.view();
        let mut matches = view
            .params
            .iter()
            .enumerate()
            .filter(|(_, item)| item.key == key);
        let (index, item) = matches
            .next()
            .ok_or_else(|| anyhow!("unknown species parameter {key:?}"))?;
        ensure!(
            matches.next().is_none(),
            "species parameter {key:?} is ambiguous"
        );
        let value = param_value(&item.kind, value).ok_or_else(|| {
            anyhow!("species parameter {key:?}: {value} out of range or wrong type")
        })?;
        let before = view.error;
        self.species.edit(ParamEdit::Set(index, value));
        let after = self.species.view().error;
        if after != before
            && let Some(error) = after
        {
            bail!("species parameter {key:?}: {error}");
        }
        Ok(())
    }
}

fn workspace_named(name: &str) -> Result<Workspace> {
    Workspace::NAMES
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(name))
        .map(|index| Workspace::ALL[index])
        .ok_or_else(|| anyhow!("unknown workspace {name:?}"))
}

fn tab_named(name: &str) -> Result<Tab> {
    let plain = |text: &str| text.replace(['-', ' ', '_'], "").to_ascii_lowercase();
    Tab::ALL
        .into_iter()
        .find(|tab| plain(tab.title()) == plain(name))
        .ok_or_else(|| anyhow!("unknown panel tab {name:?}"))
}

/// `value` as a parameter of `kind`, if it has the right type and range.
fn param_value(kind: &ParamKind, value: &Value) -> Option<ParamValue> {
    match *kind {
        ParamKind::Float { min, max, .. } => value
            .as_f64()
            .filter(|v| v.is_finite() && (min..=max).contains(v))
            .map(ParamValue::Float),
        ParamKind::Int { min, max } => value
            .as_i64()
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|v| v.fract() == 0.0)
                    .map(|v| v as i64)
            })
            .filter(|v| (min..=max).contains(v))
            .map(ParamValue::Int),
        ParamKind::Choice { options } => match value {
            Value::String(name) => options.iter().position(|option| option == name),
            _ => value
                .as_u64()
                .map(|v| v as usize)
                .filter(|v| *v < options.len()),
        }
        .map(ParamValue::Choice),
        ParamKind::Bool => value.as_bool().map(ParamValue::Bool),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_and_values_resolve() {
        assert_eq!(workspace_named("flora").unwrap(), Workspace::Flora);
        assert!(workspace_named("garden").is_err());
        assert_eq!(tab_named("line-up").unwrap(), Tab::LineUp);
        assert_eq!(tab_named("lineup").unwrap(), Tab::LineUp);
        let float = ParamKind::Float {
            min: 0.0,
            max: 2.0,
            log: false,
        };
        assert_eq!(
            param_value(&float, &json!(1.5)),
            Some(ParamValue::Float(1.5))
        );
        assert_eq!(param_value(&float, &json!(3)), None);
        let choice = ParamKind::Choice {
            options: &["a", "b"],
        };
        assert_eq!(
            param_value(&choice, &json!("b")),
            Some(ParamValue::Choice(1))
        );
        assert_eq!(param_value(&choice, &json!(2)), None);
        assert_eq!(
            param_value(&ParamKind::Int { min: 1, max: 9 }, &json!(4.0)),
            Some(ParamValue::Int(4))
        );
        assert_eq!(
            param_value(&ParamKind::Bool, &json!(true)),
            Some(ParamValue::Bool(true))
        );
    }
}
