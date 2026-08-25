use super::*;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffortPreset;
use pretty_assertions::assert_eq;

fn model(
    family: &str,
    efforts: Vec<ReasoningEffort>,
    input_modalities: Vec<InputModality>,
) -> ModelPreset {
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
        input_modalities,
    }
}

fn text_only(family: &str, efforts: Vec<ReasoningEffort>) -> ModelPreset {
    model(family, efforts, vec![InputModality::Text])
}

#[test]
fn routes_exceptional_work_to_sol_max() {
    let input = vec![UserInput::Text {
        text: "Investigate this ambiguous distributed cache invalidation failure across the codebase. Here is the stack trace: panic".to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = vec![
        text_only(
            "terra",
            vec![ReasoningEffort::Medium, ReasoningEffort::High],
        ),
        text_only("sol", vec![ReasoningEffort::High, ReasoningEffort::Max]),
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

    assert_eq!(decision.model, "gpt-5.6-sol");
    assert_eq!(decision.effort, ReasoningEffort::Max);
    assert_eq!(decision.route_class, RouteClass::Exceptional);
    assert!(!decision.used_fallback);
}

#[test]
fn routine_work_prefers_catalog_terra_medium() {
    let input = vec![UserInput::Text {
        text: "Rename this variable and run formatting.".to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = vec![
        text_only(
            "terra",
            vec![ReasoningEffort::Medium, ReasoningEffort::High],
        ),
        text_only("sol", vec![ReasoningEffort::High, ReasoningEffort::Max]),
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

    assert_eq!(decision.model, "gpt-5.6-terra");
    assert_eq!(decision.effort, ReasoningEffort::Medium);
    assert_eq!(decision.route_class, RouteClass::Routine);
    assert!(!decision.used_fallback);
}

#[test]
fn image_input_never_routes_to_text_only_model() {
    let input = vec![UserInput::Image {
        image_url: "data:image/png;base64,AA==".to_string(),
        detail: None,
    }];
    let catalog = vec![
        text_only("terra", vec![ReasoningEffort::Medium]),
        model(
            "sol",
            vec![ReasoningEffort::High],
            vec![InputModality::Text, InputModality::Image],
        ),
    ];

    let decision = CatalogModelRouter
        .route(&ModelRouterRequest {
            input: &input,
            catalog: &catalog,
            fallback_model: "gpt-5.6-terra",
            fallback_effort: Some(ReasoningEffort::Medium),
            has_previous_turn: false,
        })
        .expect("image-capable fallback should resolve");

    assert_eq!(decision.model, "gpt-5.6-sol");
    assert_eq!(decision.effort, ReasoningEffort::High);
    assert!(decision.used_fallback);
}

#[test]
fn unsupported_target_effort_uses_nearest_supported_effort() {
    let input = vec![UserInput::Text {
        text: "Investigate this ambiguous architecture migration across the entire codebase; traceback panic unknown root cause".to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = vec![
        text_only("terra", vec![ReasoningEffort::Medium]),
        text_only("sol", vec![ReasoningEffort::High]),
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

    assert_eq!(decision.model, "gpt-5.6-sol");
    assert_eq!(decision.effort, ReasoningEffort::High);
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
fn router_failure_uses_real_safe_fallback() {
    let input = vec![UserInput::Text {
        text: "anything".to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = vec![text_only("terra", vec![ReasoningEffort::Medium])];

    let decision = route_or_fallback(
        &FailingRouter,
        &ModelRouterRequest {
            input: &input,
            catalog: &catalog,
            fallback_model: "gpt-5.6-terra",
            fallback_effort: Some(ReasoningEffort::Medium),
            has_previous_turn: false,
        },
    );

    assert_eq!(decision.model, "gpt-5.6-terra");
    assert_eq!(decision.effort, ReasoningEffort::Medium);
    assert!(decision.used_fallback);
}
