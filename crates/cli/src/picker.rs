//! Arrow-key menus for choosing backend → model → effort.

use uritheking_core::catalog::{self, ModelOption};
use uritheking_core::{Result, Selection};
use inquire::{InquireError, Select};

const DEFAULT_LABEL: &str = "default";

/// Walks the user through the three choices. `Ok(None)` means they pressed Esc.
pub fn pick(current: &Selection) -> Result<Option<Selection>> {
    let backends: Vec<&str> = catalog::BACKENDS.to_vec();
    let start = backends.iter().position(|b| *b == current.backend).unwrap_or(0);
    let Some(backend) = ask(Select::new("Backend", backends).with_starting_cursor(start))? else {
        return Ok(None);
    };

    let models = catalog::models_for(backend)?;
    if models.is_empty() {
        return Err(format!("{backend} lists no models").into());
    }
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    let start = current.model.as_deref().and_then(|m| ids.iter().position(|id| *id == m)).unwrap_or(0);
    let prompt = Select::new("Model (type to filter)", ids).with_starting_cursor(start).with_page_size(12);
    let Some(model_id) = ask(prompt)? else {
        return Ok(None);
    };
    let model = models.iter().find(|m| m.id == model_id).expect("picked from this list");

    let Some(effort) = pick_effort(model, current.effort.as_deref())? else {
        return Ok(None);
    };
    Ok(Some(Selection { backend: backend.to_string(), model: Some(model.id.clone()), effort }))
}

/// `Ok(Some(None))` = model default; skipped entirely when the model has no effort levels.
fn pick_effort(model: &ModelOption, current: Option<&str>) -> Result<Option<Option<String>>> {
    if model.efforts.is_empty() {
        return Ok(Some(None));
    }
    let mut options = vec![DEFAULT_LABEL];
    options.extend(model.efforts.iter().map(String::as_str));
    let start = current.and_then(|c| options.iter().position(|o| *o == c)).unwrap_or(0);
    Ok(ask(Select::new("Effort", options).with_starting_cursor(start))?
        .map(|e| (e != DEFAULT_LABEL).then(|| e.to_string())))
}

/// Runs one menu, mapping Esc / Ctrl-C to `None`.
fn ask<T: std::fmt::Display>(select: Select<T>) -> Result<Option<T>> {
    match select.prompt() {
        Ok(v) => Ok(Some(v)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
