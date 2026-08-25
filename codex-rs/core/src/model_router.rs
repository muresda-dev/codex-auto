//! Catalog-backed local routing for the experimental Auto model-selection mode.
//!
//! Router v3 deliberately separates task profiling, model-tier selection, and
//! reasoning-effort selection. It predicts the marginal value of moving from
//! Luna -> Terra and Terra -> Sol instead of treating prompt length as a proxy
//! for difficulty. Routing stays local and deterministic: no extra model call
//! is spent just to classify a user turn.

use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::MODEL_SPECIALTY_CYBER;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::user_input::UserInput;

const TERRA_OVER_LUNA_THRESHOLD: u8 = 50;
const SOL_OVER_TERRA_THRESHOLD: u8 = 48;
const LOW_CONFIDENCE_THRESHOLD: u8 = 55;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelRouteDecision {
    pub(crate) model: String,
    pub(crate) effort: ReasoningEffort,
    pub(crate) route_class: RouteClass,
    /// 0..=100 estimate of how clearly the deterministic policy preferred the
    /// selected tier. It is a routing confidence score, not a model-quality
    /// probability.
    pub(crate) confidence: u8,
    /// 0..=100 estimated marginal value of Terra over Luna for this turn.
    pub(crate) terra_over_luna_gain: u8,
    /// 0..=100 estimated marginal value of Sol over Terra for this turn.
    pub(crate) sol_over_terra_gain: u8,
    /// Compact human-readable factors used by `/route` and telemetry.
    pub(crate) signals: Vec<String>,
    /// True when continuation hysteresis kept the previous model tier.
    pub(crate) inherited_previous: bool,
    /// True when a retry/failure signal escalated the previous model tier.
    pub(crate) escalated_retry: bool,
    /// True when the preferred family was unavailable/incompatible and the
    /// router had to use another catalog-backed model.
    pub(crate) used_fallback: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelRouterRequest<'a> {
    pub(crate) input: &'a [UserInput],
    pub(crate) catalog: &'a [ModelPreset],
    /// The currently configured model. On continued Auto sessions this also
    /// acts as a cheap previous-route hint for hysteresis.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ModelTier {
    Luna,
    Terra,
    Sol,
}

impl ModelTier {
    fn preferred_family(self) -> &'static str {
        match self {
            Self::Luna => "luna",
            Self::Terra => "terra",
            Self::Sol => "sol",
        }
    }

    fn upgrade(self) -> Self {
        match self {
            Self::Luna => Self::Terra,
            Self::Terra | Self::Sol => Self::Sol,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Routine,
    Implementation,
    Debugging,
    Review,
    Architecture,
    Migration,
    Research,
    Security,
}

impl TaskKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Routine => "routine",
            Self::Implementation => "implementation",
            Self::Debugging => "debugging",
            Self::Review => "review",
            Self::Architecture => "architecture",
            Self::Migration => "migration",
            Self::Research => "research",
            Self::Security => "security",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    Local,
    File,
    MultiFile,
    Repository,
    System,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::File => "file",
            Self::MultiFile => "multi-file",
            Self::Repository => "repository",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

impl Risk {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    fn model_floor(self) -> Option<ModelTier> {
        match self {
            Self::Low => None,
            Self::Medium => Some(ModelTier::Terra),
            Self::High | Self::Critical => Some(ModelTier::Sol),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
    Mechanical,
    Moderate,
    Deep,
    Exceptional,
}

#[derive(Debug, Clone)]
struct TaskProfile {
    kind: TaskKind,
    scope: Scope,
    risk: Risk,
    depth: Depth,
    ambiguity: u8,
    verification_need: u8,
    context_pressure: u8,
    continuation: bool,
    retry: bool,
    error_material: bool,
    non_text_items: u8,
    signals: Vec<String>,
}

impl TaskProfile {
    fn from_request(request: &ModelRouterRequest<'_>) -> Self {
        let (text, non_text_items) = collect_input(request.input);
        // Unicode-aware lowercasing matters for Russian prompts. The v2 router
        // used ASCII-only lowercasing, which missed uppercase Cyrillic signals.
        let normalized = text.to_lowercase();
        let char_count = text.chars().count();
        let line_count = text.lines().count();
        let code_blocks = text.matches("```").count() / 2;

        let error_material = contains_any(
            &normalized,
            &[
                "stack trace",
                "traceback",
                "panic",
                "exception",
                "error[",
                "segfault",
                "deadlock",
                "race condition",
                "ошибка",
                "падает",
                "трейс",
                "гонк",
                "дедлок",
            ],
        );
        let security = contains_any(
            &normalized,
            &[
                "security",
                "vulnerability",
                "authentication",
                "authorization",
                "permission",
                "credential",
                "secret",
                "encryption",
                "oauth",
                "passkey",
                "безопасност",
                "уязвим",
                "аутентифик",
                "авторизац",
                "прав доступа",
                "учетн",
                "секрет",
                "шифрован",
            ],
        );
        let migration = contains_any(
            &normalized,
            &[
                "migration",
                "migrate",
                "schema change",
                "zero downtime",
                "without downtime",
                "миграц",
                "перенос данных",
                "смена схем",
                "без простоя",
            ],
        );
        let architecture = contains_any(
            &normalized,
            &[
                "architecture",
                "architect",
                "system design",
                "distributed system",
                "trade-off",
                "tradeoff",
                "scalability",
                "horizontal scaling",
                "архитект",
                "проектирован",
                "распределенн",
                "компромисс",
                "масштабирован",
            ],
        );
        let review = contains_any(
            &normalized,
            &[
                "code review",
                "review this",
                "audit",
                "find issues",
                "проведи ревью",
                "код-ревью",
                "аудит",
                "найди проблемы",
            ],
        );
        let research = contains_any(
            &normalized,
            &[
                "research",
                "compare approaches",
                "evaluate options",
                "investigate alternatives",
                "исследуй",
                "сравни подход",
                "оцени вариант",
                "сравни вариант",
            ],
        );
        let debugging = error_material
            || contains_any(
                &normalized,
                &[
                    "debug",
                    "root cause",
                    "why does",
                    "why is",
                    "fix this bug",
                    "investigate failure",
                    "отлад",
                    "причина ошибки",
                    "почему падает",
                    "найди причину",
                    "исправь баг",
                ],
            );
        let implementation = contains_any(
            &normalized,
            &[
                "implement",
                "refactor",
                "rewrite",
                "add feature",
                "create endpoint",
                "build ",
                "реализуй",
                "рефактор",
                "перепиши",
                "добавь функцион",
                "создай endpoint",
                "создай эндпоинт",
            ],
        );
        let mechanical = contains_any(
            &normalized,
            &[
                "rename",
                "format this",
                "run formatting",
                "replace every",
                "replace all",
                "bump version",
                "переименуй",
                "отформатируй",
                "замени везде",
                "замени все",
                "обнови версию",
            ],
        ) && !architecture
            && !migration
            && !security
            && !debugging;

        let kind = if security {
            TaskKind::Security
        } else if migration {
            TaskKind::Migration
        } else if architecture {
            TaskKind::Architecture
        } else if debugging {
            TaskKind::Debugging
        } else if review {
            TaskKind::Review
        } else if research {
            TaskKind::Research
        } else if implementation {
            TaskKind::Implementation
        } else {
            TaskKind::Routine
        };

        let system_scope = contains_any(
            &normalized,
            &[
                "system-wide",
                "whole system",
                "distributed system",
                "100k",
                "100 000",
                "100000",
                "million users",
                "horizontal scaling",
                "production architecture",
                "вся система",
                "100 тысяч",
                "миллион пользователей",
                "горизонтальн",
                "архитектура saas",
            ],
        );
        let repository_scope = contains_any(
            &normalized,
            &[
                "entire repo",
                "entire repository",
                "whole repo",
                "whole project",
                "across the codebase",
                "all modules",
                "repo-wide",
                "весь репозитор",
                "весь проект",
                "по всему проекту",
                "все модули",
            ],
        );
        let multi_file_scope = contains_any(
            &normalized,
            &[
                "multiple files",
                "several files",
                "several modules",
                "across files",
                "несколько файлов",
                "несколько модул",
            ],
        );
        let file_scope = contains_any(
            &normalized,
            &["this file", "in the file", "этот файл", "в файле"],
        );
        let scope = if system_scope {
            Scope::System
        } else if repository_scope {
            Scope::Repository
        } else if multi_file_scope {
            Scope::MultiFile
        } else if file_scope {
            Scope::File
        } else {
            Scope::Local
        };

        let uncertainty_markers = count_groups(
            &normalized,
            &[
                &["ambiguous", "uncertain", "unknown", "unclear"],
                &["figure out", "best approach", "best way", "choose between"],
                &["неопредел", "неизвест", "неясн", "непонятн"],
                &["лучший подход", "лучший вариант", "выбери между"],
            ],
        );
        let ambiguity = (uncertainty_markers * 22 + u8::from(architecture) * 10).min(100);

        let compare_alternatives = contains_any(
            &normalized,
            &[
                "compare",
                "trade-off",
                "tradeoff",
                "alternatives",
                "pros and cons",
                "сравни",
                "компромисс",
                "альтернатив",
                "плюсы и минусы",
            ],
        );
        let asks_for_verification = contains_any(
            &normalized,
            &[
                "test",
                "tests",
                "benchmark",
                "prove",
                "verify",
                "edge case",
                "тест",
                "бенчмарк",
                "докажи",
                "проверь",
                "краев",
            ],
        );
        let verification_need = if compare_alternatives || asks_for_verification || review {
            80
        } else if debugging || implementation || migration {
            50
        } else {
            20
        };

        let destructive = contains_any(
            &normalized,
            &[
                "drop table",
                "drop column",
                "delete data",
                "truncate",
                "irreversible",
                "data loss",
                "удали данные",
                "удалить данные",
                "drop column",
                "потеря данных",
                "необратим",
            ],
        );
        let production = contains_any(
            &normalized,
            &[
                "production",
                " prod ",
                "zero downtime",
                "without downtime",
                "live traffic",
                "боевой",
                "продакш",
                "проде",
                "без простоя",
                "без остановки",
            ],
        );
        let payments = contains_any(
            &normalized,
            &[
                "payment",
                "billing",
                "checkout",
                "money",
                "оплат",
                "платеж",
                "деньг",
            ],
        );
        let credentials = contains_any(
            &normalized,
            &[
                "credential",
                "secret",
                "private key",
                "token rotation",
                "парол",
                "секрет",
                "приватн ключ",
                "ротац токен",
            ],
        );
        let risk = if (destructive && production) || (security && credentials && production) {
            Risk::Critical
        } else if destructive || production || security || payments {
            Risk::High
        } else if migration {
            Risk::Medium
        } else {
            Risk::Low
        };

        let deep_reasoning = architecture
            || migration
            || security
            || compare_alternatives
            || contains_any(
                &normalized,
                &[
                    "root cause",
                    "reason step by step",
                    "optimize",
                    "constraint",
                    "инвариант",
                    "ограничени",
                    "оптимиз",
                    "причин",
                ],
            );
        let depth = if error_material
            && ambiguity >= 40
            && matches!(scope, Scope::Repository | Scope::System)
        {
            Depth::Exceptional
        } else if deep_reasoning {
            Depth::Deep
        } else if mechanical {
            Depth::Mechanical
        } else {
            Depth::Moderate
        };

        let continuation = request.has_previous_turn
            && char_count <= 220
            && contains_any(
                &normalized,
                &[
                    "continue",
                    "go ahead",
                    "do it",
                    "proceed",
                    "implement it",
                    "fix it",
                    "yes",
                    "продолж",
                    "дальше",
                    "делай",
                    "сделай",
                    "реализуй",
                    "исправь",
                    "да",
                    "ок",
                    "теперь",
                ],
            );
        let retry = request.has_previous_turn
            && contains_any(
                &normalized,
                &[
                    "still failing",
                    "still broken",
                    "doesn't work",
                    "does not work",
                    "didn't work",
                    "failed again",
                    "try again",
                    "retry",
                    "не работает",
                    "не помог",
                    "снова падает",
                    "опять ошибка",
                    "всё ещё",
                    "все еще",
                    "попробуй снова",
                ],
            );

        let mut context_pressure = 0_u8;
        context_pressure = context_pressure.saturating_add(if char_count > 12_000 {
            50
        } else if char_count > 4_000 {
            30
        } else if char_count > 1_500 {
            15
        } else {
            0
        });
        context_pressure = context_pressure.saturating_add(if line_count > 80 {
            20
        } else if line_count > 25 {
            10
        } else {
            0
        });
        context_pressure = context_pressure.saturating_add((code_blocks.min(3) * 8) as u8);
        context_pressure = context_pressure.saturating_add(non_text_items.saturating_mul(12));
        context_pressure = context_pressure.min(100);

        let mut signals = vec![
            format!("kind:{}", kind.as_str()),
            format!("scope:{}", scope.as_str()),
            format!("risk:{}", risk.as_str()),
        ];
        if mechanical {
            signals.push("mechanical".to_string());
        }
        if error_material {
            signals.push("error-material".to_string());
        }
        if compare_alternatives {
            signals.push("alternatives".to_string());
        }
        if ambiguity >= 40 {
            signals.push("ambiguous".to_string());
        }
        if context_pressure >= 30 {
            signals.push("large-context".to_string());
        }
        if non_text_items > 0 {
            signals.push("multimodal".to_string());
        }
        if continuation {
            signals.push("continuation".to_string());
        }
        if retry {
            signals.push("retry".to_string());
        }

        Self {
            kind,
            scope,
            risk,
            depth,
            ambiguity,
            verification_need,
            context_pressure,
            continuation,
            retry,
            error_material,
            non_text_items,
            signals,
        }
    }

    fn marginal_gains(&self) -> (u8, u8) {
        let mut terra = 12_i16;
        let mut sol = 4_i16;

        let (terra_kind, sol_kind) = match self.kind {
            TaskKind::Routine => (0, 0),
            TaskKind::Implementation => (28, 10),
            TaskKind::Debugging => (32, 18),
            TaskKind::Review => (24, 18),
            TaskKind::Architecture => (40, 42),
            TaskKind::Migration => (42, 46),
            TaskKind::Research => (28, 30),
            TaskKind::Security => (35, 45),
        };
        terra += terra_kind;
        sol += sol_kind;

        let (terra_scope, sol_scope) = match self.scope {
            Scope::Local => (0, 0),
            Scope::File => (4, 1),
            Scope::MultiFile => (8, 8),
            Scope::Repository => (12, 18),
            Scope::System => (16, 22),
        };
        terra += terra_scope;
        sol += sol_scope;

        let (terra_depth, sol_depth) = match self.depth {
            Depth::Mechanical => (-10, -10),
            Depth::Moderate => (6, 4),
            Depth::Deep => (14, 18),
            Depth::Exceptional => (20, 30),
        };
        terra += terra_depth;
        sol += sol_depth;

        terra += i16::from(self.ambiguity / 8);
        sol += i16::from(self.ambiguity / 5);
        terra += i16::from(self.verification_need / 20);
        sol += i16::from(self.verification_need / 12);
        // Context volume is intentionally only a weak signal: a 10k-line
        // mechanical rename should not become Sol merely because it is long.
        terra += i16::from(self.context_pressure / 25);
        sol += i16::from(self.context_pressure / 20);

        match self.risk {
            Risk::Low => {}
            Risk::Medium => {
                terra += 5;
                sol += 7;
            }
            Risk::High => {
                terra += 8;
                sol += 18;
            }
            Risk::Critical => {
                terra += 10;
                sol += 28;
            }
        }

        if self.error_material {
            terra += 5;
            sol += 6;
        }
        if self.non_text_items > 0 {
            terra += 3;
            sol += 3;
        }
        if self.retry {
            terra += 10;
            sol += 18;
        }

        (clamp_score(terra), clamp_score(sol))
    }

    fn reasoning_need(&self) -> u8 {
        let mut need = match self.kind {
            TaskKind::Routine => 18_i16,
            TaskKind::Implementation => 38,
            TaskKind::Debugging => 48,
            TaskKind::Review => 45,
            TaskKind::Architecture => 55,
            TaskKind::Migration => 58,
            TaskKind::Research => 50,
            TaskKind::Security => 58,
        };
        need += match self.scope {
            Scope::Local => 0,
            Scope::File => 3,
            Scope::MultiFile => 6,
            Scope::Repository => 10,
            Scope::System => 12,
        };
        need += match self.depth {
            Depth::Mechanical => -10,
            Depth::Moderate => 4,
            Depth::Deep => 12,
            Depth::Exceptional => 28,
        };
        need += i16::from(self.ambiguity / 6);
        need += i16::from(self.verification_need / 12);
        need += match self.risk {
            Risk::Low => 0,
            Risk::Medium => 4,
            Risk::High => 8,
            Risk::Critical => 14,
        };
        need += i16::from(self.context_pressure / 20);
        if self.error_material {
            need += 6;
        }
        if self.retry {
            need += 8;
        }
        clamp_score(need)
    }
}

/// Deterministic, capability-aware Auto policy.
pub(crate) struct CatalogModelRouter;

impl ModelRouter for CatalogModelRouter {
    fn route(
        &self,
        request: &ModelRouterRequest<'_>,
    ) -> Result<ModelRouteDecision, ModelRouterError> {
        let profile = TaskProfile::from_request(request);
        let (terra_gain, sol_gain) = profile.marginal_gains();
        let previous_tier = request
            .has_previous_turn
            .then(|| tier_for_model(request.fallback_model))
            .flatten();

        let mut tier = if sol_gain >= SOL_OVER_TERRA_THRESHOLD {
            ModelTier::Sol
        } else if terra_gain >= TERRA_OVER_LUNA_THRESHOLD {
            ModelTier::Terra
        } else {
            ModelTier::Luna
        };

        let mut inherited_previous = false;
        let mut escalated_retry = false;

        if let Some(floor) = profile.risk.model_floor()
            && tier < floor
        {
            tier = floor;
        }

        if profile.continuation
            && let Some(previous_tier) = previous_tier
            && tier < previous_tier
        {
            tier = previous_tier;
            inherited_previous = true;
        }

        if profile.retry {
            let retry_floor = previous_tier.map_or(ModelTier::Terra, ModelTier::upgrade);
            if tier < retry_floor {
                tier = retry_floor;
            }
            escalated_retry = true;
        }

        let confidence = route_confidence(tier, terra_gain, sol_gain, &profile);
        // Selective-classification style defer: only ambiguous, genuinely
        // low-confidence cases are promoted. Borderline ordinary work is not
        // automatically made expensive.
        if confidence < LOW_CONFIDENCE_THRESHOLD && profile.ambiguity >= 40 {
            tier = tier.upgrade();
        }

        let requirements = InputRequirements::from_input(request.input);
        let (selected, used_fallback, used_specialty) = select_catalog_model(
            request,
            requirements,
            tier,
            matches!(profile.kind, TaskKind::Security),
        )
        .ok_or(ModelRouterError)?;

        let mut target_effort = effort_for_need(profile.reasoning_need());
        if profile.risk >= Risk::High {
            target_effort = max_known_effort(target_effort, ReasoningEffort::High);
        }
        if (profile.continuation || profile.retry)
            && let Some(previous_effort) = request.fallback_effort.clone()
        {
            target_effort = max_known_effort(target_effort, previous_effort);
        }
        if profile.retry && tier == ModelTier::Sol && previous_tier == Some(ModelTier::Sol) {
            target_effort = max_known_effort(target_effort, ReasoningEffort::Max);
        }

        let effort = closest_supported_effort(selected, &target_effort)
            .or_else(|| default_effort(selected))
            .ok_or(ModelRouterError)?;
        let route_class = route_class_for(tier, &effort, &profile);

        let mut signals = profile.signals.clone();
        signals.push(format!("gain:luna-terra={terra_gain}"));
        signals.push(format!("gain:terra-sol={sol_gain}"));
        if inherited_previous {
            signals.push("hysteresis:previous-tier".to_string());
        }
        if escalated_retry {
            signals.push("escalation:retry".to_string());
        }
        if used_specialty {
            signals.push("specialty:cyber".to_string());
        }
        if confidence < LOW_CONFIDENCE_THRESHOLD && profile.ambiguity >= 40 {
            signals.push("defer:low-confidence".to_string());
        }

        Ok(ModelRouteDecision {
            model: selected.model.clone(),
            effort,
            route_class,
            confidence,
            terra_over_luna_gain: terra_gain,
            sol_over_terra_gain: sol_gain,
            signals,
            inherited_previous,
            escalated_retry,
            used_fallback,
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
    let profile = TaskProfile::from_request(request);
    let (terra_gain, sol_gain) = profile.marginal_gains();
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
            request.catalog.iter().find(|model| {
                model.is_default && is_selectable(model) && requirements.supported_by(model)
            })
        })
        .or_else(|| {
            request
                .catalog
                .iter()
                .find(|model| is_selectable(model) && requirements.supported_by(model))
        });

    let route_class = if profile.risk >= Risk::High {
        RouteClass::Complex
    } else {
        RouteClass::Analytical
    };
    let mut signals = profile.signals;
    signals.push("router:fallback".to_string());

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
            confidence: 0,
            terra_over_luna_gain: terra_gain,
            sol_over_terra_gain: sol_gain,
            signals,
            inherited_previous: false,
            escalated_retry: false,
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
        confidence: 0,
        terra_over_luna_gain: terra_gain,
        sol_over_terra_gain: sol_gain,
        signals,
        inherited_previous: false,
        escalated_retry: false,
        used_fallback: true,
    }
}

fn select_catalog_model<'a>(
    request: &'a ModelRouterRequest<'_>,
    requirements: InputRequirements,
    tier: ModelTier,
    prefer_cyber_specialty: bool,
) -> Option<(&'a ModelPreset, bool, bool)> {
    if prefer_cyber_specialty
        && let Some(model) = request.catalog.iter().find(|model| {
            model.model_specialty.as_deref() == Some(MODEL_SPECIALTY_CYBER)
                && is_selectable(model)
                && requirements.supported_by(model)
        })
    {
        return Some((model, false, true));
    }

    for (index, family) in family_order(tier).iter().enumerate() {
        if let Some(model) = request.catalog.iter().find(|model| {
            is_selectable(model)
                && requirements.supported_by(model)
                && belongs_to_family(model, family)
        }) {
            return Some((model, index != 0, false));
        }
    }

    request
        .catalog
        .iter()
        .find(|model| {
            model.model == request.fallback_model
                && is_selectable(model)
                && requirements.supported_by(model)
        })
        .or_else(|| {
            request.catalog.iter().find(|model| {
                model.is_default && is_selectable(model) && requirements.supported_by(model)
            })
        })
        .or_else(|| {
            request
                .catalog
                .iter()
                .find(|model| is_selectable(model) && requirements.supported_by(model))
        })
        .map(|model| (model, true, false))
}

fn family_order(tier: ModelTier) -> &'static [&'static str] {
    match tier {
        ModelTier::Luna => &["luna", "terra", "sol"],
        ModelTier::Terra => &["terra", "sol", "luna"],
        ModelTier::Sol => &["sol", "terra", "luna"],
    }
}

fn tier_for_model(model: &str) -> Option<ModelTier> {
    if model
        .rsplit('-')
        .next()
        .is_some_and(|family| family.eq_ignore_ascii_case("luna"))
    {
        Some(ModelTier::Luna)
    } else if model
        .rsplit('-')
        .next()
        .is_some_and(|family| family.eq_ignore_ascii_case("terra"))
    {
        Some(ModelTier::Terra)
    } else if model
        .rsplit('-')
        .next()
        .is_some_and(|family| family.eq_ignore_ascii_case("sol"))
    {
        Some(ModelTier::Sol)
    } else {
        None
    }
}

fn route_confidence(tier: ModelTier, terra_gain: u8, sol_gain: u8, profile: &TaskProfile) -> u8 {
    let margin = match tier {
        ModelTier::Luna => TERRA_OVER_LUNA_THRESHOLD.saturating_sub(terra_gain),
        ModelTier::Terra => {
            let lower = terra_gain.saturating_sub(TERRA_OVER_LUNA_THRESHOLD);
            let upper = SOL_OVER_TERRA_THRESHOLD.saturating_sub(sol_gain);
            lower.min(upper)
        }
        ModelTier::Sol => sol_gain.saturating_sub(SOL_OVER_TERRA_THRESHOLD),
    };
    let mut confidence = 56_i16 + i16::from(margin.min(18)) * 2;
    confidence += i16::try_from(profile.signals.len().min(6)).unwrap_or(0);
    confidence -= i16::from(profile.ambiguity / 6);
    if profile.continuation {
        confidence -= 4;
    }
    clamp_score(confidence).clamp(35, 98)
}

fn route_class_for(tier: ModelTier, effort: &ReasoningEffort, profile: &TaskProfile) -> RouteClass {
    if effort_rank(effort).is_some_and(|rank| rank >= 6)
        || matches!(profile.depth, Depth::Exceptional)
    {
        RouteClass::Exceptional
    } else {
        match tier {
            ModelTier::Luna => RouteClass::Routine,
            ModelTier::Terra => RouteClass::Analytical,
            ModelTier::Sol => RouteClass::Complex,
        }
    }
}

fn effort_for_need(need: u8) -> ReasoningEffort {
    match need {
        0..=24 => ReasoningEffort::Low,
        25..=54 => ReasoningEffort::Medium,
        55..=91 => ReasoningEffort::High,
        _ => ReasoningEffort::Max,
    }
}

fn max_known_effort(left: ReasoningEffort, right: ReasoningEffort) -> ReasoningEffort {
    match (effort_rank(&left), effort_rank(&right)) {
        (Some(left_rank), Some(right_rank)) if right_rank > left_rank => right,
        _ => left,
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

fn collect_input(input: &[UserInput]) -> (String, u8) {
    let mut text = String::new();
    let mut non_text_items = 0_u8;
    for item in input {
        match item {
            UserInput::Text {
                text: item_text, ..
            } => {
                text.push_str(item_text);
                text.push('\n');
            }
            UserInput::Skill { name, .. } | UserInput::Mention { name, .. } => {
                text.push_str(name);
                text.push('\n');
            }
            _ => non_text_items = non_text_items.saturating_add(1),
        }
    }
    (text, non_text_items)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn count_groups(text: &str, groups: &[&[&str]]) -> u8 {
    groups
        .iter()
        .filter(|group| contains_any(text, group))
        .count()
        .try_into()
        .unwrap_or(u8::MAX)
}

fn clamp_score(value: i16) -> u8 {
    value.clamp(0, 100) as u8
}

#[cfg(test)]
#[path = "model_router_tests.rs"]
mod tests;
