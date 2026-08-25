//! Catalog-backed local routing for the experimental Auto model-selection mode.
//!
//! The router uses only the already loaded model catalog and the current user
//! turn. It never spends an extra model request just to classify a prompt.
//! Model cost is observed after the turn through Codex telemetry and is kept
//! separate from the synchronous routing decision.

use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::user_input::UserInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteClass {
    Routine,
    Analytical,
    Complex,
    Exceptional,
}

impl RouteClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Routine => "routine",
            Self::Analytical => "analytical",
            Self::Complex => "complex",
            Self::Exceptional => "exceptional",
        }
    }

    fn target(self) -> (&'static str, ReasoningEffort) {
        match self {
            Self::Routine => ("terra", ReasoningEffort::Medium),
            Self::Analytical => ("terra", ReasoningEffort::High),
            Self::Complex => ("sol", ReasoningEffort::High),
            Self::Exceptional => ("sol", ReasoningEffort::Max),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelRouteDecision {
    pub(crate) model: String,
    pub(crate) effort: ReasoningEffort,
    pub(crate) route_class: RouteClass,
    /// True when the preferred family was unavailable/incompatible and the
    /// router had to use a safe catalog fallback.
    pub(crate) used_fallback: bool,
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

pub(crate) trait ModelRouter: Send + Sync {
    fn route(
        &self,
        request: &ModelRouterRequest<'_>,
    ) -> Result<ModelRouteDecision, ModelRouterError>;
}

/// Deterministic, capability-aware Auto policy.
///
/// `terra` and `sol` remain preferences rather than hard requirements. If the
/// backend catalog changes, Auto falls back to a compatible real model instead
/// of manufacturing a model id or failing the turn.
pub(crate) struct CatalogModelRouter;

impl ModelRouter for CatalogModelRouter {
    fn route(
        &self,
        request: &ModelRouterRequest<'_>,
    ) -> Result<ModelRouteDecision, ModelRouterError> {
        let route_class = PromptComplexity::from_request(request);
        let (preferred_family, target_effort) = route_class.target();
        let requirements = InputRequirements::from_input(request.input);

        let preferred = request.catalog.iter().find(|model| {
            is_selectable(model)
                && requirements.supported_by(model)
                && belongs_to_family(model, preferred_family)
        });

        let selected = preferred
            .or_else(|| {
                request.catalog.iter().find(|model| {
                    model.model == request.fallback_model
                        && is_selectable(model)
                        && requirements.supported_by(model)
                })
            })
            .or_else(|| {
                request
                    .catalog
                    .iter()
                    .find(|model| model.is_default && is_selectable(model) && requirements.supported_by(model))
            })
            .or_else(|| {
                request
                    .catalog
                    .iter()
                    .find(|model| is_selectable(model) && requirements.supported_by(model))
            })
            .ok_or(ModelRouterError)?;

        let effort = closest_supported_effort(selected, &target_effort)
            .or_else(|| default_effort(selected))
            .ok_or(ModelRouterError)?;

        Ok(ModelRouteDecision {
            model: selected.model.clone(),
            effort,
            route_class,
            used_fallback: preferred.is_none(),
        })
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
    let route_class = PromptComplexity::from_request(request);
    let requirements = InputRequirements::from_input(request.input);
    let candidate = request
        .catalog
        .iter()
        .find(|model| {
            model.model == request.fallback_model
                && is_selectable(model)
                && requirements.supported_by(model)
        })
        .or_else(|| {
            request
                .catalog
                .iter()
                .find(|model| model.is_default && is_selectable(model) && requirements.supported_by(model))
        })
        .or_else(|| {
            request
                .catalog
                .iter()
                .find(|model| is_selectable(model) && requirements.supported_by(model))
        });

    if let Some(model) = candidate {
        let effort = request
            .fallback_effort
            .as_ref()
            .and_then(|effort| closest_supported_effort(model, effort))
            .or_else(|| default_effort(model))
            .unwrap_or(ReasoningEffort::Medium);
        return ModelRouteDecision {
            model: model.model.clone(),
            effort,
            route_class,
            used_fallback: true,
        };
    }

    ModelRouteDecision {
        model: request.fallback_model.to_string(),
        effort: request
            .fallback_effort
            .clone()
            .unwrap_or(ReasoningEffort::Medium),
        route_class,
        used_fallback: true,
    }
}

fn is_selectable(model: &ModelPreset) -> bool {
    model.show_in_picker && model.supported_in_api
}

fn belongs_to_family(model: &ModelPreset, family: &str) -> bool {
    model
        .model
        .rsplit('-')
        .next()
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(family))
}

fn default_effort(model: &ModelPreset) -> Option<ReasoningEffort> {
    closest_supported_effort(model, &model.default_reasoning_effort).or_else(|| {
        model
            .supported_reasoning_efforts
            .first()
            .map(|preset| preset.effort.clone())
    })
}

/// Pick the exact requested effort when available, otherwise the nearest known
/// effort advertised by the model. On an equal distance we prefer the lower
/// effort, avoiding unnecessary reasoning cost.
fn closest_supported_effort(
    model: &ModelPreset,
    target: &ReasoningEffort,
) -> Option<ReasoningEffort> {
    if model
        .supported_reasoning_efforts
        .iter()
        .any(|preset| &preset.effort == target)
    {
        return Some(target.clone());
    }

    let target_rank = effort_rank(target)?;
    model
        .supported_reasoning_efforts
        .iter()
        .filter_map(|preset| {
            effort_rank(&preset.effort).map(|rank| {
                (
                    (rank - target_rank).unsigned_abs(),
                    rank,
                    preset.effort.clone(),
                )
            })
        })
        .min_by_key(|(distance, rank, _)| (*distance, *rank))
        .map(|(_, _, effort)| effort)
}

fn effort_rank(effort: &ReasoningEffort) -> Option<i16> {
    match effort {
        ReasoningEffort::None => Some(0),
        ReasoningEffort::Minimal => Some(1),
        ReasoningEffort::Low => Some(2),
        ReasoningEffort::Medium => Some(3),
        ReasoningEffort::High => Some(4),
        ReasoningEffort::XHigh => Some(5),
        ReasoningEffort::Max => Some(6),
        ReasoningEffort::Ultra => Some(7),
        ReasoningEffort::Custom(_) => None,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct InputRequirements {
    text: bool,
    image: bool,
    audio: bool,
}

impl InputRequirements {
    fn from_input(input: &[UserInput]) -> Self {
        let mut requirements = Self::default();
        for item in input {
            match item {
                UserInput::Text { .. } | UserInput::Skill { .. } | UserInput::Mention { .. } => {
                    requirements.text = true;
                }
                UserInput::Image { .. } | UserInput::LocalImage { .. } => {
                    requirements.image = true;
                }
                UserInput::Audio { .. } | UserInput::LocalAudio { .. } => {
                    requirements.audio = true;
                }
                _ => requirements.text = true,
            }
        }
        requirements
    }

    fn supported_by(self, model: &ModelPreset) -> bool {
        (!self.text || model.input_modalities.contains(&InputModality::Text))
            && (!self.image || model.input_modalities.contains(&InputModality::Image))
            && (!self.audio || model.input_modalities.contains(&InputModality::Audio))
    }
}

struct PromptComplexity;

impl PromptComplexity {
    fn from_request(request: &ModelRouterRequest<'_>) -> RouteClass {
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
            0..=1 => RouteClass::Routine,
            2..=3 => RouteClass::Analytical,
            4..=6 => RouteClass::Complex,
            _ => RouteClass::Exceptional,
        }
    }
}

#[cfg(test)]
#[path = "model_router_tests.rs"]
mod tests;
