use super::*;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffortPreset;
use pretty_assertions::assert_eq;

fn model(family: &str, efforts: Vec<ReasoningEffort>) -> ModelPreset {
    ModelPreset {
        id: format!("gpt-5.6-{family}"),
        model: format!("gpt-5.6-{family}"),
        display_name: family.to_string(),
        description: String::new(),
        model_specialty: None,
        default_reasoning_effort: ReasoningEffort::Medium,
        supported_reasoning_efforts: efforts
            .into_iter()
            .map(|effort| ReasoningEffortPreset {
                effort,
                description: String::new(),
            })
            .collect(),
        supports_personality: false,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        is_default: family == "terra",
        upgrade: None,
        show_in_picker: true,
        multi_agent_version: None,
        availability_nux: None,
        supported_in_api: true,
        input_modalities: vec![InputModality::Text],
    }
}

#[test]
fn routes_different_complexity_classes_to_different_catalog_models() {
    let input = vec![UserInput::Text {
        text: "Investigate this ambiguous distributed cache invalidation failure across the codebase. Here is the stack trace: panic".to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = vec![
        model(
            "terra",
            vec![ReasoningEffort::Medium, ReasoningEffort::High],
        ),
        model("sol", vec![ReasoningEffort::High, ReasoningEffort::Max]),
    ];
    let decision = CatalogModelRouter
        .route(&ModelRouterRequest {
            input: &input,
            catalog: &catalog,
            fallback_model: "gpt-5.6-terra",
            fallback_effort: Some(ReasoningEffort::Medium),
            has_previous_turn: true,
        })
        .expect("route should resolve");

    assert_eq!(
        decision,
        ModelRouteDecision {
            model: "gpt-5.6-sol".to_string(),
            effort: ReasoningEffort::Max,
        }
    );
}

#[test]
fn routine_work_prefers_catalog_terra_medium() {
    let input = vec![UserInput::Text {
        text: "Rename this variable and run formatting.".to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = vec![
        model(
            "terra",
            vec![ReasoningEffort::Medium, ReasoningEffort::High],
        ),
        model("sol", vec![ReasoningEffort::High, ReasoningEffort::Max]),
    ];
    let decision = CatalogModelRouter
        .route(&ModelRouterRequest {
            input: &input,
            catalog: &catalog,
            fallback_model: "gpt-5.6-terra",
            fallback_effort: Some(ReasoningEffort::Medium),
            has_previous_turn: false,
        })
        .expect("route should resolve");

    assert_eq!(
        decision,
        ModelRouteDecision {
            model: "gpt-5.6-terra".to_string(),
            effort: ReasoningEffort::Medium,
        }
    );
}

struct FailingRouter;

impl ModelRouter for FailingRouter {
    fn route(
        &self,
        _request: &ModelRouterRequest<'_>,
    ) -> Result<ModelRouteDecision, ModelRouterError> {
        Err(ModelRouterError)
    }
}

#[test]
fn router_failure_uses_the_real_safe_fallback() {
    let input = vec![UserInput::Text {
        text: "anything".to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = vec![model("terra", vec![ReasoningEffort::Medium])];

    assert_eq!(
        route_or_fallback(
            &FailingRouter,
            &ModelRouterRequest {
                input: &input,
                catalog: &catalog,
                fallback_model: "gpt-5.6-terra",
                fallback_effort: Some(ReasoningEffort::Medium),
                has_previous_turn: false,
            },
        ),
        ModelRouteDecision {
            model: "gpt-5.6-terra".to_string(),
            effort: ReasoningEffort::Medium,
        }
    );
}
