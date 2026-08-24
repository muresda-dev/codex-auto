//! Catalog-backed local routing for the experimental Auto model-selection mode.
//!
//! The router works only from the already loaded catalog and user-turn state.
//! It never sends a classifier request before the real turn.

use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::user_input::UserInput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelRouteDecision {
    pub(crate) model: String,
    pub(crate) effort: ReasoningEffort,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelRouterRequest<'a> {
    pub(crate) input: &'a [UserInput],
    pub(crate) catalog: &'a [ModelPreset],
    pub(crate) fallback_model: &'a str,
    pub(crate) fallback_effort: Option<ReasoningEffort>,
    pub(crate) has_previous_turn: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelRouterError;

/// Chooses a real catalog model and supported reasoning effort for one user
/// turn. Implementations must not return synthetic/local model identifiers.
pub(crate) trait ModelRouter: Send + Sync {
    fn route(
        &self,
        request: &ModelRouterRequest<'_>,
    ) -> Result<ModelRouteDecision, ModelRouterError>;
}

/// The initial deterministic Auto policy.
///
/// It combines task-shape evidence (input volume, code and error material,
/// requested scope, and uncertainty) instead of relying on a single keyword
/// list. The catalog remains authoritative for candidates and effort levels.
pub(crate) struct CatalogModelRouter;

impl ModelRouter for CatalogModelRouter {
    fn route(
        &self,
        request: &ModelRouterRequest<'_>,
    ) -> Result<ModelRouteDecision, ModelRouterError> {
        let (family, effort) = match PromptComplexity::from_request(request) {
            Complexity::Routine => ("terra", ReasoningEffort::Medium),
            Complexity::Analytical => ("terra", ReasoningEffort::High),
            Complexity::Complex => ("sol", ReasoningEffort::High),
            Complexity::Exceptional => ("sol", ReasoningEffort::Max),
        };

        preferred_model(request.catalog, family)
            .and_then(|model| supported_decision(model, effort))
            .ok_or(ModelRouterError)
    }
}

pub(crate) fn route_or_fallback(
    router: &impl ModelRouter,
    request: &ModelRouterRequest<'_>,
) -> ModelRouteDecision {
    router
        .route(request)
        .unwrap_or_else(|_| fallback_decision(request))
}

fn fallback_decision(request: &ModelRouterRequest<'_>) -> ModelRouteDecision {
    request
        .catalog
        .iter()
        .find(|model| model.model == request.fallback_model)
        .and_then(|model| {
            request
                .fallback_effort
                .clone()
                .and_then(|effort| supported_decision(model, effort))
                .or_else(|| default_decision(model))
        })
        .or_else(|| {
            request
                .catalog
                .iter()
                .find(|model| model.is_default)
                .and_then(default_decision)
        })
        .or_else(|| request.catalog.first().and_then(default_decision))
        .unwrap_or_else(|| ModelRouteDecision {
            model: request.fallback_model.to_string(),
            effort: request
                .fallback_effort
                .clone()
                .unwrap_or(ReasoningEffort::Medium),
        })
}

fn preferred_model<'a>(catalog: &'a [ModelPreset], family: &str) -> Option<&'a ModelPreset> {
    catalog.iter().find(|model| {
        model.show_in_picker
            && model.supported_in_api
            && model
                .model
                .rsplit('-')
                .next()
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(family))
    })
}

fn supported_decision(model: &ModelPreset, effort: ReasoningEffort) -> Option<ModelRouteDecision> {
    model
        .supported_reasoning_efforts
        .iter()
        .any(|preset| preset.effort == effort)
        .then(|| ModelRouteDecision {
            model: model.model.clone(),
            effort,
        })
}

fn default_decision(model: &ModelPreset) -> Option<ModelRouteDecision> {
    supported_decision(model, model.default_reasoning_effort.clone()).or_else(|| {
        model
            .supported_reasoning_efforts
            .first()
            .map(|preset| ModelRouteDecision {
                model: model.model.clone(),
                effort: preset.effort.clone(),
            })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Complexity {
    Routine,
    Analytical,
    Complex,
    Exceptional,
}

struct PromptComplexity;

impl PromptComplexity {
    fn from_request(request: &ModelRouterRequest<'_>) -> Complexity {
        let mut text = String::new();
        let mut non_text_items = 0_u8;
        for item in request.input {
            match item {
                UserInput::Text {
                    text: item_text, ..
                } => {
                    text.push_str(item_text);
                    text.push('\n');
                }
                _ => non_text_items = non_text_items.saturating_add(1),
            }
        }

        let normalized = text.to_ascii_lowercase();
        let lines = text.lines().count();
        let code_blocks = text.matches("```").count() / 2;
        let has_error_material = normalized.contains("stack trace")
            || normalized.contains("traceback")
            || normalized.contains("panic")
            || normalized.contains("exception")
            || normalized.contains("error[")
            || normalized.contains("ошибка")
            || normalized.contains("падает");
        let asks_for_analysis = normalized.contains("analyze")
            || normalized.contains("investigate")
            || normalized.contains("root cause")
            || normalized.contains("проанализ")
            || normalized.contains("исследуй");
        let asks_for_architecture = normalized.contains("architecture")
            || normalized.contains("migration")
            || normalized.contains("trade-off")
            || normalized.contains("design")
            || normalized.contains("архитект")
            || normalized.contains("миграц")
            || normalized.contains("компромисс");
        let has_high_uncertainty = normalized.contains("uncertain")
            || normalized.contains("ambiguous")
            || normalized.contains("unknown")
            || normalized.contains("неопредел")
            || normalized.contains("неизвест");
        let asks_for_large_scope = normalized.contains("entire")
            || normalized.contains("across the codebase")
            || normalized.contains("all modules")
            || normalized.contains("whole project")
            || normalized.contains("весь репозитор")
            || normalized.contains("все модул");

        let mut score = 0_u8;
        score += u8::from(text.len() > 1_500);
        score += u8::from(lines > 25);
        score += u8::from(code_blocks > 0);
        score += u8::from(non_text_items > 0);
        score += u8::from(asks_for_analysis);
        score += u8::from(has_error_material) * 2;
        score += u8::from(asks_for_architecture) * 2;
        score += u8::from(has_high_uncertainty) * 2;
        score += u8::from(asks_for_large_scope) * 2;
        score += u8::from(request.has_previous_turn && (has_error_material || asks_for_analysis));

        match score {
            0..=1 => Complexity::Routine,
            2..=3 => Complexity::Analytical,
            4..=6 => Complexity::Complex,
            _ => Complexity::Exceptional,
        }
    }
}

#[cfg(test)]
#[path = "model_router_tests.rs"]
mod tests;
