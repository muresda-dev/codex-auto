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

fn standard_catalog() -> Vec<ModelPreset> {
    vec![
        text_only("luna", vec![ReasoningEffort::Low, ReasoningEffort::Medium]),
        text_only(
            "terra",
            vec![ReasoningEffort::Medium, ReasoningEffort::High],
        ),
        text_only("sol", vec![ReasoningEffort::High, ReasoningEffort::Max]),
    ]
}

fn route_text(
    text: &str,
    fallback_model: &str,
    fallback_effort: Option<ReasoningEffort>,
    has_previous_turn: bool,
) -> ModelRouteDecision {
    let input = vec![UserInput::Text {
        text: text.to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = standard_catalog();
    CatalogModelRouter
        .route(&ModelRouterRequest {
            input: &input,
            catalog: &catalog,
            fallback_model,
            fallback_effort,
            has_previous_turn,
        })
        .expect("route should resolve")
}

#[test]
fn greeting_prefers_luna() {
    let decision = route_text("Привет!", "gpt-5.6-terra", None, false);
    assert_eq!(decision.model, "gpt-5.6-luna");
    assert_eq!(decision.route_class, RouteClass::Routine);
    assert!(matches!(
        decision.effort,
        ReasoningEffort::Low | ReasoningEffort::Medium
    ));
    assert!(decision.terra_over_luna_gain < TERRA_OVER_LUNA_THRESHOLD);
}

#[test]
fn mechanical_long_prompt_stays_luna() {
    let filler = " value".repeat(2_500);
    let prompt =
        format!("Rename this variable and run formatting. Do not change behavior.{filler}");
    let decision = route_text(&prompt, "gpt-5.6-terra", None, false);
    assert_eq!(decision.model, "gpt-5.6-luna");
    assert!(decision.signals.iter().any(|signal| signal == "mechanical"));
    assert!(
        decision
            .signals
            .iter()
            .any(|signal| signal == "large-context")
    );
}

#[test]
fn normal_multi_file_implementation_prefers_terra() {
    let decision = route_text(
        "Implement this feature across several files and add tests for the new endpoint.",
        "gpt-5.6-terra",
        None,
        false,
    );
    assert_eq!(decision.model, "gpt-5.6-terra");
    assert!(matches!(
        decision.effort,
        ReasoningEffort::Medium | ReasoningEffort::High
    ));
}

#[test]
fn medium_explanations_and_technology_comparisons_prefer_terra() {
    let prompts = [
        "Объясни, почему в Django возникает проблема N+1 запросов. Сравни select_related и prefetch_related, приведи примеры, когда использовать каждый вариант, и назови типичные ошибки.",
        "Сравни Celery и обычные фоновые задачи через asyncio для Django-приложения. Объясни преимущества и ограничения каждого подхода и в каких ситуациях ты выбрал бы один вместо другого.",
    ];

    for prompt in prompts {
        let decision = route_text(prompt, "gpt-5.6-terra", None, false);
        assert_eq!(
            decision.model, "gpt-5.6-terra",
            "medium analytical prompt should route to Terra: {prompt}; signals={:?}; terra_gain={}; sol_gain={}",
            decision.signals, decision.terra_over_luna_gain, decision.sol_over_terra_gain
        );
        assert!(decision.terra_over_luna_gain >= TERRA_OVER_LUNA_THRESHOLD);
        assert!(decision.sol_over_terra_gain < SOL_OVER_TERRA_THRESHOLD);
    }
}

#[test]
fn local_implementation_and_review_have_terra_floor_by_value() {
    let prompts = [
        "Реализуй в этом файле функцию нормализации данных и добавь тесты для основных случаев.",
        "Проведи код-ревью этого файла, найди потенциальные ошибки и объясни замечания.",
    ];

    for prompt in prompts {
        let decision = route_text(prompt, "gpt-5.6-terra", None, false);
        assert_eq!(
            decision.model, "gpt-5.6-terra",
            "ordinary engineering work should route to Terra: {prompt}; signals={:?}; terra_gain={}; sol_gain={}",
            decision.signals, decision.terra_over_luna_gain, decision.sol_over_terra_gain
        );
    }
}

#[test]
fn real_world_architecture_prompt_routes_to_sol_high() {
    let decision = route_text(
        "Спроектируй архитектуру SaaS на Django для 100 тысяч активных пользователей: PostgreSQL, Redis, Celery, WebSocket, горизонтальное масштабирование. Сравни несколько вариантов архитектуры, укажи компромиссы, риски и предложи план миграции без простоя.",
        "gpt-5.6-terra",
        Some(ReasoningEffort::Medium),
        true,
    );
    assert_eq!(decision.model, "gpt-5.6-sol");
    assert_eq!(decision.effort, ReasoningEffort::High);
    assert_eq!(decision.route_class, RouteClass::Complex);
    assert!(decision.sol_over_terra_gain >= SOL_OVER_TERRA_THRESHOLD);
}

#[test]
fn exceptional_ambiguous_repo_failure_routes_to_sol_max() {
    let decision = route_text(
        "Investigate this ambiguous distributed cache invalidation failure across the codebase. The root cause is unknown. Here is the stack trace: panic. Compare alternatives and verify edge cases.",
        "gpt-5.6-terra",
        Some(ReasoningEffort::High),
        true,
    );
    assert_eq!(decision.model, "gpt-5.6-sol");
    assert_eq!(decision.effort, ReasoningEffort::Max);
    assert_eq!(decision.route_class, RouteClass::Exceptional);
}

#[test]
fn high_risk_short_prompt_has_sol_floor() {
    let decision = route_text(
        "Rotate production credentials and change authentication without downtime.",
        "gpt-5.6-terra",
        Some(ReasoningEffort::Medium),
        false,
    );
    assert_eq!(decision.model, "gpt-5.6-sol");
    assert!(effort_rank(&decision.effort).unwrap_or_default() >= 4);
    assert!(
        decision
            .signals
            .iter()
            .any(|signal| signal == "risk:critical" || signal == "risk:high")
    );
}

#[test]
fn short_continuation_inherits_previous_sol_tier() {
    let decision = route_text(
        "Да, продолжай и сделай это.",
        "gpt-5.6-sol",
        Some(ReasoningEffort::High),
        true,
    );
    assert_eq!(decision.model, "gpt-5.6-sol");
    assert!(decision.inherited_previous);
    assert!(effort_rank(&decision.effort).unwrap_or_default() >= 4);
}

#[test]
fn explicit_retry_escalates_previous_terra_to_sol() {
    let decision = route_text(
        "Всё ещё не работает, попробуй снова. Опять ошибка.",
        "gpt-5.6-terra",
        Some(ReasoningEffort::High),
        true,
    );
    assert_eq!(decision.model, "gpt-5.6-sol");
    assert!(decision.escalated_retry);
}

#[test]
fn image_input_never_routes_to_text_only_model() {
    let input = vec![UserInput::Image {
        image_url: "data:image/png;base64,AA==".to_string(),
        detail: None,
    }];
    let catalog = vec![
        text_only("luna", vec![ReasoningEffort::Low]),
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
fn missing_luna_falls_back_to_terra_for_routine_work() {
    let input = vec![UserInput::Text {
        text: "Привет!".to_string(),
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
            has_previous_turn: false,
        })
        .expect("fallback should resolve");
    assert_eq!(decision.model, "gpt-5.6-terra");
    assert!(decision.used_fallback);
}

#[test]
fn unsupported_target_effort_uses_nearest_supported_effort() {
    let input = vec![UserInput::Text {
        text: "Investigate this ambiguous architecture migration across the entire repository; traceback panic unknown root cause".to_string(),
        text_elements: Vec::new(),
    }];
    let catalog = vec![
        text_only("luna", vec![ReasoningEffort::Low]),
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
    assert_eq!(decision.confidence, 0);
    assert!(decision.used_fallback);
}

#[test]
fn routing_regression_benchmark_covers_180_prompt_variants() {
    let routine = [
        "Привет!",
        "Объясни, что делает эта переменная.",
        "Rename this variable and run formatting.",
        "Summarize this short comment.",
        "Переименуй локальную переменную.",
    ];
    let terra = [
        "Implement this feature across several files and add tests.",
        "Debug this error in several files and find the root cause.",
        "Проведи код-ревью нескольких файлов и найди проблемы.",
        "Реализуй функционал в нескольких модулях и добавь тесты.",
        "Compare approaches for this local implementation and verify tests.",
    ];
    let sol = [
        "Design the system architecture for horizontal scaling and compare trade-offs.",
        "Plan a zero downtime production database migration and explain risks.",
        "Спроектируй архитектуру всей системы, сравни варианты и компромиссы.",
        "Найди причину неизвестного падения по всему репозиторию: traceback panic; сравни альтернативы.",
        "Design authentication and permission architecture for production credentials.",
    ];
    let neutral_suffixes = [
        "",
        " Please be concise.",
        " Ответ дай по-русски.",
        " Explain the result clearly.",
        " Не меняй публичный API без необходимости.",
        " Use the existing coding style.",
        " Keep backward compatibility.",
        " Сохрани обратную совместимость.",
        " Include a short summary.",
        " Do not invent unavailable APIs.",
        " Проверь результат внимательно.",
        " Follow the repository conventions.",
    ];

    let mut cases = 0_usize;
    for suffix in neutral_suffixes {
        for prompt in routine {
            let decision = route_text(&format!("{prompt}{suffix}"), "gpt-5.6-terra", None, false);
            assert_eq!(
                tier_for_model(&decision.model),
                Some(ModelTier::Luna),
                "routine case routed unexpectedly: {prompt}{suffix}"
            );
            cases += 1;
        }
        for prompt in terra {
            let decision = route_text(&format!("{prompt}{suffix}"), "gpt-5.6-terra", None, false);
            assert_eq!(
                tier_for_model(&decision.model),
                Some(ModelTier::Terra),
                "terra case routed unexpectedly: {prompt}{suffix}; signals={:?}",
                decision.signals
            );
            cases += 1;
        }
        for prompt in sol {
            let decision = route_text(&format!("{prompt}{suffix}"), "gpt-5.6-terra", None, false);
            assert_eq!(
                tier_for_model(&decision.model),
                Some(ModelTier::Sol),
                "sol case routed unexpectedly: {prompt}{suffix}; signals={:?}",
                decision.signals
            );
            cases += 1;
        }
    }

    assert_eq!(cases, 180);
}
