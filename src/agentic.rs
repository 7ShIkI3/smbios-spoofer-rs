//! # Patterns Agentiques pour le Spoofer SMBIOS
//!
//! Ce module implémente les patterns fondamentaux des frameworks agentic
//! (LangGraph, CrewAI, AutoGen, OpenAI Agents SDK, Claude Agent SDK)
//! adaptés pour le bare-metal UEFI.
//!
//! ## Pourquoi ces patterns dans un spoofer UEFI ?
//!
//! Le flux de spoofing est fondamentalement un **agentic loop** :
//!   1. **Observe** — lire l'état actuel des tables SMBIOS
//!   2. **Decide** — choisir une stratégie (agressif, furtif, équilibré)
//!   3. **Act** — exécuter les patchs via des outils
//!   4. **Persist** — sauvegarder l'état pour le prochain cycle
//!
//! Chaque étape est un "agent" spécialisé avec ses propres outils,
//! stratégies de retry et audit trail.
//!
//! ## Patterns implémentés
//!
//! | Pattern        | GitHub Origin           | Adaptation UEFI                  |
//! |----------------|------------------------|----------------------------------|
//! | Agent + Runner | CrewAI, AutoGen         | Cycle de vie d'un agent spoofer  |
//! | ToolRegistry   | LangGraph, Claude SDK   | Dispatch des opérations patch    |
//! | Pipeline       | LangGraph, AutoGen      | Chaîne find→load→patch→persist   |
//! | AuditTrail     | Tous les frameworks     | Journal immutable des opérations |
//! | Strategy       | TDT / CrewAI personas   | Modes Narcissism/Psycho/Mach     |
//! | StateMachine   | LangGraph               | États formels du flux de boot    |
//!
//! ## Mapping avec The Dark Triad
//!
//! Le projet TDT définit 3 personnalités. Ce module les implémente
//! comme des stratégies de spoofing interchangeables :
//!
//! - **Narcissism** (🪞) → AggressiveStrategy : change tout, format libre,
//!   maximisation des modifications, zéro discrétion
//! - **Psychopathy** (🔪) → RelentlessStrategy : essaie TOUS les patchs,
//!   retry infini, aucune limite, ignore les erreurs
//! - **Machiavellianism** (🕸️) → StealthStrategy : changements minimaux,
//!   préserve le format original, indétectable, auto-protection

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::boxed::Box;
use alloc::format;

// ─── Couleurs TDT (pour les logs) ──────────────────────────────────────────

/// Couleurs associées aux personnalités TDT.
/// Utilisées dans l'affichage console et l'audit trail.
pub mod colors {
    pub const NARCISSISM: &str = "🪞";   // yellow
    pub const PSYCHOPATHY: &str = "🔪";  // red
    pub const MACHIAVELLIANISM: &str = "🕸️"; // cyan
    pub const BALANCED: &str = "⚖️";     // white
}

// ─── Agent ─────────────────────────────────────────────────────────────────

/// Les trois personnalités du Dark Triad.
/// Chacune a des comportements d'exécution différents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonalityMode {
    /// Maximum de changements, ignore le format original, zéro discrétion.
    /// Biais outil : prend le plus puissant en premier.
    /// Confirmation : auto-approve tout.
    Narcissism,

    /// Essaie tous les patchs, retry infini, ignore les erreurs non-fatales.
    /// Biais outil : utilise TOUS les outils disponibles.
    /// Confirmation : auto-approve tout.
    Psychopathy,

    /// Changements minimaux, préserve le format original, indétectable.
    /// Biais outil : choisit l'outil le plus précis/furtif.
    /// Confirmation : vérifie avant d'agir si l'opération est risquée.
    Machiavellianism,

    /// Mélange équilibré des trois personnalités (fusion engine TDT).
    Balanced,
}

impl PersonalityMode {
    /// Niveau d'agressivité (0-100).
    pub fn aggression(&self) -> u8 {
        match self {
            Self::Narcissism => 100,
            Self::Psychopathy => 85,
            Self::Machiavellianism => 20,
            Self::Balanced => 50,
        }
    }

    /// Niveau de furtivité (0-100).
    pub fn stealth(&self) -> u8 {
        match self {
            Self::Narcissism => 5,
            Self::Psychopathy => 0,
            Self::Machiavellianism => 100,
            Self::Balanced => 60,
        }
    }

    /// Niveau de persistance (0-100) — capacité à continuer malgré les échecs.
    pub fn persistence(&self) -> u8 {
        match self {
            Self::Narcissism => 20,   // abandonne vite
            Self::Psychopathy => 100, // ne s'arrête jamais
            Self::Machiavellianism => 80,
            Self::Balanced => 60,
        }
    }

    /// Emoji associé à la personnalité.
    pub fn emoji(&self) -> &str {
        match self {
            Self::Narcissism => colors::NARCISSISM,
            Self::Psychopathy => colors::PSYCHOPATHY,
            Self::Machiavellianism => colors::MACHIAVELLIANISM,
            Self::Balanced => colors::BALANCED,
        }
    }

    /// Nombre max de retries par défaut pour cette personnalité.
    pub fn default_max_retries(&self) -> usize {
        match self {
            Self::Narcissism => 1,     // pas de retry, passe à autre chose
            Self::Psychopathy => 10,   // retry agressif
            Self::Machiavellianism => 3, // retry stratégique
            Self::Balanced => 3,
        }
    }

    /// Nombre max d'étapes avant abandon.
    pub fn default_max_steps(&self) -> usize {
        match self {
            Self::Narcissism => 5,     // impatient
            Self::Psychopathy => 20,   // va jusqu'au bout
            Self::Machiavellianism => 10,
            Self::Balanced => 8,
        }
    }
}

/// Contexte partagé entre tous les agents pendant l'exécution.
///
/// Contient l'état mutable de la session d'exécution :
/// état actuel, compteur d'étapes, erreurs rencontrées.
pub struct AgentContext {
    /// Nombre d'étapes exécutées
    pub step_count: usize,
    /// État actuel de l'agent
    pub state: AgentState,
    /// Messages/observations accumulés
    pub observations: Vec<String>,
    /// Erreurs rencontrées (non-fatales)
    pub errors: Vec<String>,
    /// Données arbitraires partagées entre les étapes
    pub data: AgentData,
    /// Niveau de confiance actuel (0.0 - 1.0)
    pub confidence: f32,
}

impl AgentContext {
    pub fn new() -> Self {
        Self {
            step_count: 0,
            state: AgentState::Idle,
            observations: Vec::new(),
            errors: Vec::new(),
            data: AgentData::default(),
            confidence: 0.0,
        }
    }

    /// Enregistre une observation (texte libre).
    pub fn observe(&mut self, msg: &str) {
        self.observations.push(String::from(msg));
    }

    /// Enregistre une erreur non-fatale.
    pub fn record_error(&mut self, err: &str) {
        self.errors.push(String::from(err));
        self.confidence *= 0.8; // baisse de confiance
    }

    /// Vérifie si on a dépassé le budget d'erreurs.
    pub fn error_budget_exceeded(&self, max_errors: usize) -> bool {
        self.errors.len() >= max_errors
    }

    /// Vérifie si on a dépassé le budget d'étapes.
    pub fn step_budget_exceeded(&self, max_steps: usize) -> bool {
        self.step_count >= max_steps
    }
}

impl Default for AgentContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Données partagées entre les étapes d'un agent.
/// Clé-valeur simple pour le passage d'information.
#[derive(Default)]
pub struct AgentData {
    entries: Vec<(String, String)>,
}

impl AgentData {
    pub fn set(&mut self, key: &str, value: &str) {
        // Écrase si la clé existe déjà
        if let Some(pos) = self.entries.iter().position(|(k, _)| k == key) {
            self.entries[pos] = (String::from(key), String::from(value));
        } else {
            self.entries.push((String::from(key), String::from(value)));
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// États possibles d'un agent pendant son cycle de vie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Observing,
    Deciding,
    Acting,
    Done,
    Failed,
    Aborted,
}

/// Résultat de l'exécution d'un agent.
#[derive(Debug)]
pub enum AgentResult {
    Success { steps: usize, final_message: String },
    Failed { step: usize, reason: String },
    Aborted { step: usize, reason: String },
    Timeout { step: usize, max_steps: usize },
}

/// Trait Agent — l'unité d'exécution autonome.
///
/// Un agent est une entité qui :
/// 1. Observe l'environnement (`observe`)
/// 2. Décide de la prochaine action (`decide`)
/// 3. Exécute l'action via ses outils (`act`)
/// 4. Recommence jusqu'à terminaison
///
/// Ce pattern est le cœur des frameworks agentic (CrewAI, AutoGen, LangGraph).
pub trait Agent {
    /// Nom de l'agent (pour les logs et l'audit trail).
    fn name(&self) -> &str;

    /// Description de ce que fait l'agent.
    fn description(&self) -> &str;

    /// Personnalité de l'agent (mode TDT).
    fn personality(&self) -> PersonalityMode;

    /// Phase d'observation : l'agent inspecte son environnement.
    /// Retourne les observations faites (diagnostic, état actuel).
    fn observe(&mut self, ctx: &mut AgentContext);

    /// Phase de décision : l'agent choisit la prochaine action.
    /// Retourne le nom de l'outil à exécuter et ses paramètres.
    fn decide(&mut self, ctx: &mut AgentContext) -> Option<(String, ToolParams)>;

    /// Phase d'action : exécute l'outil choisi.
    fn act(
        &mut self,
        tool_name: &str,
        params: &ToolParams,
        ctx: &mut AgentContext,
    ) -> ToolResult;

    /// Vérifie si l'agent a terminé sa mission.
    fn is_done(&self, ctx: &AgentContext) -> bool;

    /// Nombre max d'étapes avant abandon forcé.
    fn max_steps(&self) -> usize {
        self.personality().default_max_steps()
    }
}

/// Exécuteur d'agent — implémente la boucle agentic standard.
///
/// La boucle canonique (identique à LangGraph/OpenAI Agents SDK) :
/// ```text
/// while !done && step < max_steps:
///     agent.observe(ctx)
///     if let Some((tool, params)) = agent.decide(ctx):
///         result = agent.act(tool, params, ctx)
///         audit.record(result)
///         if result.is_err():
///             handle_error()
///     step += 1
/// ```
pub struct AgentRunner {
    max_steps: usize,
    max_retries: usize,
    max_errors: usize,
    audit: AuditTrail,
    personality: PersonalityMode,
}

impl AgentRunner {
    /// Crée un runner avec les paramètres par défaut pour la personnalité donnée.
    pub fn new(personality: PersonalityMode) -> Self {
        Self {
            max_steps: personality.default_max_steps(),
            max_retries: personality.default_max_retries(),
            max_errors: 10,
            audit: AuditTrail::new(),
            personality,
        }
    }

    /// Crée un runner avec des paramètres personnalisés.
    pub fn with_limits(
        personality: PersonalityMode,
        max_steps: usize,
        max_retries: usize,
        max_errors: usize,
    ) -> Self {
        Self {
            max_steps,
            max_retries,
            max_errors,
            audit: AuditTrail::new(),
            personality,
        }
    }

    /// Exécute un agent jusqu'à terminaison.
    ///
    /// C'est LA boucle agentic standard. Elle implémente le pattern
    /// Observe → Decide → Act → Observe → ... trouvé dans TOUS les
    /// frameworks agentic (LangGraph, CrewAI, OpenAI Agents, Claude SDK).
    pub fn run(&mut self, agent: &mut dyn Agent) -> AgentResult {
        let mut ctx = AgentContext::new();
        let agent_name = String::from(agent.name());
        let personality = agent.personality();

        self.audit.record(
            AuditLevel::Info,
            &agent_name,
            "agent_start",
            &format!("{} Starting agent: {}", personality.emoji(), agent.description()),
        );

        while !agent.is_done(&ctx) {
            // ── Vérification des budgets ──────────────────────────────────
            if ctx.step_budget_exceeded(self.max_steps) {
                self.audit.record(
                    AuditLevel::Warn,
                    &agent_name,
                    "budget_exceeded",
                    &format!("Step budget exceeded: {} steps", ctx.step_count),
                );
                return AgentResult::Timeout {
                    step: ctx.step_count,
                    max_steps: self.max_steps,
                };
            }

            if ctx.error_budget_exceeded(self.max_errors) {
                self.audit.record(
                    AuditLevel::Error,
                    &agent_name,
                    "error_budget_exceeded",
                    &format!("Error budget exceeded: {} errors", ctx.errors.len()),
                );
                return AgentResult::Failed {
                    step: ctx.step_count,
                    reason: format!("Error budget exceeded: {} errors", ctx.errors.len()),
                };
            }

            // ── Phase OBSERVE ─────────────────────────────────────────────
            ctx.state = AgentState::Observing;
            agent.observe(&mut ctx);

            // ── Phase DECIDE ──────────────────────────────────────────────
            ctx.state = AgentState::Deciding;
            let action = match agent.decide(&mut ctx) {
                Some(a) => a,
                None => {
                    // Rien à faire — l'agent considère sa mission terminée
                    // si aucune action n'est décidée
                    if ctx.step_count > 0 {
                        // L'agent a déjà agi, on considère que c'est fini
                        break;
                    }
                    // Aucune action décidée sans avoir agi → échec
                    return AgentResult::Failed {
                        step: ctx.step_count,
                        reason: String::from("Agent decided no action on first step"),
                    };
                }
            };

            // ── Phase ACT (avec retry) ────────────────────────────────────
            ctx.state = AgentState::Acting;
            let (tool_name, params) = action;
            let result = self.execute_with_retry(agent, &tool_name, &params, &mut ctx);

            match result {
                Ok(msg) => {
                    self.audit.record(
                        AuditLevel::Info,
                        &agent_name,
                        &tool_name,
                        &msg,
                    );
                    ctx.confidence = (ctx.confidence + 0.2).min(1.0);
                }
                Err(err) => {
                    self.audit.record(
                        AuditLevel::Error,
                        &agent_name,
                        &tool_name,
                        &err,
                    );
                    ctx.record_error(&err);

                    // Si c'est une erreur fatale, on arrête
                    if err.contains("FATAL") {
                        return AgentResult::Failed {
                            step: ctx.step_count,
                            reason: err,
                        };
                    }
                }
            }

            ctx.step_count += 1;
        }

        ctx.state = AgentState::Done;
        let msg = format!(
            "{} Agent '{}' completed in {} steps (confidence: {:.0}%)",
            personality.emoji(),
            agent.name(),
            ctx.step_count,
            ctx.confidence * 100.0,
        );
        self.audit.record(AuditLevel::Success, &agent_name, "agent_done", &msg);

        AgentResult::Success {
            steps: ctx.step_count,
            final_message: msg,
        }
    }

    /// Exécute une action avec retry selon la stratégie de la personnalité.
    fn execute_with_retry(
        &mut self,
        agent: &mut dyn Agent,
        tool_name: &str,
        params: &ToolParams,
        ctx: &mut AgentContext,
    ) -> Result<String, String> {
        let mut last_err = String::new();

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                ctx.observe(&format!(
                    "Retry {}/{} for '{}' after: {}",
                    attempt, self.max_retries, tool_name, last_err
                ));
            }

            let result = agent.act(tool_name, params, ctx);

            match result {
                ToolResult::Success { message, .. } => return Ok(message),
                ToolResult::PartialSuccess { message, repaired } => {
                    let status = if repaired {
                        format!("{} (auto-repaired)", message)
                    } else {
                        message
                    };
                    return Ok(status);
                }
                ToolResult::RetryableError { message, .. } => {
                    last_err = message;
                    // Continue la boucle de retry
                }
                ToolResult::FatalError { message } => {
                    return Err(message);
                }
                ToolResult::Skipped { reason } => {
                    return Ok(format!("Skipped: {}", reason));
                }
            }
        }

        Err(format!(
            "Failed after {} retries: {}",
            self.max_retries, last_err
        ))
    }

    /// Retourne une référence vers l'audit trail.
    pub fn audit(&self) -> &AuditTrail {
        &self.audit
    }

    /// Affiche le résumé d'exécution (format texte console UEFI).
    pub fn print_summary(&self) {
        let total = self.audit.entries.len();
        let errors = self.audit.entries.iter().filter(|e| e.level == AuditLevel::Error).count();
        let success = self.audit.entries.iter().filter(|e| e.level == AuditLevel::Success).count();
        uefi::println!(
            "{} AgentRunner: {} total, {} ok, {} errors",
            self.personality.emoji(),
            total,
            success,
            errors
        );
    }
}

// ─── Tool Registry ─────────────────────────────────────────────────────────

/// Paramètres passés à un outil.
/// Structure clé-valeur simple, compatible no_std.
#[derive(Debug, Clone)]
pub struct ToolParams {
    entries: Vec<(String, String)>,
}

impl ToolParams {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.entries.push((String::from(key), String::from(value)));
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn has(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }
}

impl Default for ToolParams {
    fn default() -> Self {
        Self::new()
    }
}

/// Résultat d'exécution d'un outil.
#[derive(Debug, Clone)]
pub enum ToolResult {
    /// Succès complet.
    Success { message: String, data: Option<String> },
    /// Succès partiel (ex: patch appliqué mais format dégradé).
    PartialSuccess { message: String, repaired: bool },
    /// Erreur récupérable — on peut retry.
    RetryableError { message: String, retry_after_ms: u64 },
    /// Erreur fatale — pas de retry possible.
    FatalError { message: String },
    /// Outil sautée volontairement (pas nécessaire).
    Skipped { reason: String },
}

impl ToolResult {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::RetryableError { .. } | Self::FatalError { .. })
    }

    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::FatalError { .. })
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Success { message, .. } => message,
            Self::PartialSuccess { message, .. } => message,
            Self::RetryableError { message, .. } => message,
            Self::FatalError { message } => message,
            Self::Skipped { reason } => reason,
        }
    }
}

/// Trait Tool — une opération unitaire enregistrée dans le ToolRegistry.
///
/// Chaque outil est une opération atomique qui peut être :
/// - Appelée par un agent
/// - Validée avant exécution
/// - Rollback si nécessaire
///
/// Pattern directement inspiré du `@tool_registry.register()` de LangGraph
/// et du `@function_tool` de OpenAI Agents SDK.
pub trait Tool {
    /// Nom unique de l'outil.
    fn name(&self) -> &str;

    /// Description de ce que fait l'outil.
    fn description(&self) -> &str;

    /// Exécute l'outil avec les paramètres donnés.
    fn execute(&mut self, params: &ToolParams) -> ToolResult;

    /// Valide les paramètres avant exécution.
    /// Retourne true si les paramètres sont valides.
    fn validate(&self, params: &ToolParams) -> bool {
        let _ = params;
        true
    }

    /// L'outil est-il destructif ? (nécessite confirmation humaine)
    fn is_destructive(&self) -> bool {
        false
    }

    /// Rollback de l'opération si supporté.
    /// Par défaut, pas de rollback.
    fn rollback(&mut self) -> ToolResult {
        ToolResult::Skipped {
            reason: String::from("Rollback not supported"),
        }
    }

    /// Catégorie de l'outil pour le dispatch.
    fn category(&self) -> ToolCategory {
        ToolCategory::General
    }

    /// Personnalité recommandée pour cet outil.
    /// None = neutre, convient à toutes les personnalités.
    fn recommended_personality(&self) -> Option<PersonalityMode> {
        None
    }
}

/// Catégories d'outils pour le dispatch et les biais de personnalité.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    /// Découverte / reconnaissance
    Discovery,
    /// Patch / modification mémoire
    Mutation,
    /// Persistance / sauvegarde
    Persistence,
    /// Analyse / validation
    Analysis,
    /// Général (non catégorisé)
    General,
}

/// Registre d'outils — pattern central des frameworks agentic.
///
/// Le ToolRegistry est le "catalogue" de toutes les opérations disponibles.
/// Les agents y puisent leurs capacités. Chaque personnalité TDT a des
/// biais de sélection d'outils différents.
///
/// Pattern : LangGraph `ToolNode`, OpenAI Agents SDK `tool_registry`,
/// Claude Agent SDK `ToolRegistry`.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Enregistre un nouvel outil.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Trouve un outil par nom.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// Trouve un outil mutable par nom.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut (dyn Tool + '_)> {
        for tool in self.tools.iter_mut() {
            if tool.name() == name {
                return Some(tool.as_mut());
            }
        }
        None
    }

    /// Liste les outils disponibles pour une personnalité donnée.
    ///
    /// Chaque personnalité a des biais de sélection :
    /// - Narcissism : préfère les outils puissants (destructifs)
    /// - Psychopathy : prend TOUS les outils
    /// - Machiavellianism : préfère les outils furtifs, évite les destructifs
    pub fn list_for_personality(&self, mode: PersonalityMode) -> Vec<&dyn Tool> {
        match mode {
            PersonalityMode::Psychopathy => {
                // Psychopathy : TOUS les outils, aucune discrimination
                self.tools.iter().map(|t| t.as_ref()).collect()
            }
            PersonalityMode::Machiavellianism => {
                // Machiavellianism : éviter les outils destructifs
                self.tools
                    .iter()
                    .filter(|t| !t.is_destructive())
                    .map(|t| t.as_ref())
                    .collect()
            }
            PersonalityMode::Narcissism => {
                // Narcissism : les outils destructifs d'abord, puis les autres
                let mut all: Vec<&dyn Tool> = self.tools.iter().map(|t| t.as_ref()).collect();
                all.sort_by_key(|t| {
                    if t.is_destructive() {
                        0 // priorité haute
                    } else {
                        1
                    }
                });
                all
            }
            PersonalityMode::Balanced => {
                self.tools.iter().map(|t| t.as_ref()).collect()
            }
        }
    }

    /// Nombre d'outils enregistrés.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Vrai si aucun outil.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Liste les noms de tous les outils.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Pipeline ──────────────────────────────────────────────────────────────

/// Statut d'une étape du pipeline après exécution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Ok,
    Failed,
    Skipped,
    RolledBack,
}

/// Résultat d'exécution d'une étape du pipeline.
#[derive(Debug)]
pub struct StepResult {
    pub status: StepStatus,
    pub message: String,
    pub duration_ms: u64,
}

impl StepResult {
    pub fn ok(msg: &str, duration_ms: u64) -> Self {
        Self {
            status: StepStatus::Ok,
            message: String::from(msg),
            duration_ms,
        }
    }

    pub fn fail(msg: &str, duration_ms: u64) -> Self {
        Self {
            status: StepStatus::Failed,
            message: String::from(msg),
            duration_ms,
        }
    }

    pub fn skip(msg: &str) -> Self {
        Self {
            status: StepStatus::Skipped,
            message: String::from(msg),
            duration_ms: 0,
        }
    }
}

/// Trait pour une étape de pipeline.
///
/// Une PipelineStep est l'équivalent d'un "nœud" dans LangGraph
/// ou d'une "task" dans CrewAI.
pub trait PipelineStep {
    /// Nom de l'étape.
    fn name(&self) -> &str;

    /// Exécute l'étape.
    fn execute(&mut self, ctx: &mut PipelineContext) -> StepResult;

    /// Rollback de l'étape si elle a échoué.
    /// Appelée automatiquement par le pipeline en cas d'échec.
    fn rollback(&mut self, _ctx: &mut PipelineContext) {
        // Par défaut, pas de rollback
    }

    /// Doit-on retry cette étape en cas d'échec ?
    fn should_retry(&self, result: &StepResult) -> bool {
        result.status == StepStatus::Failed
    }

    /// Nombre max de retries pour cette étape.
    fn max_retries(&self) -> usize {
        1
    }
}

/// Contexte d'exécution du pipeline.
pub struct PipelineContext {
    /// Résultats de chaque étape (ordre d'exécution).
    pub results: Vec<(String, StepResult)>,
    /// Données partagées entre les étapes.
    pub data: AgentData,
    /// Le pipeline doit-il s'arrêter ?
    pub should_stop: bool,
}

impl PipelineContext {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            data: AgentData::default(),
            should_stop: false,
        }
    }

    /// Récupère le résultat de la dernière étape exécutée.
    pub fn last_result(&self) -> Option<&StepResult> {
        self.results.last().map(|(_, r)| r)
    }

    /// Vérifie si toutes les étapes ont réussi.
    pub fn all_ok(&self) -> bool {
        self.results
            .iter()
            .all(|(_, r)| r.status == StepStatus::Ok || r.status == StepStatus::Skipped)
    }
}

impl Default for PipelineContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Pipeline d'exécution — enchaînement d'étapes avec retry et rollback.
///
/// Équivalent du `StateGraph` de LangGraph ou du `Pipeline` de CrewAI.
/// Permet de composer des étapes et de gérer les échecs de manière
/// structurée.
pub struct Pipeline {
    steps: Vec<Box<dyn PipelineStep>>,
    /// Si true, le pipeline s'arrête au premier échec.
    /// Si false, continue malgré les échecs (mode Psychopathy).
    fail_fast: bool,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            fail_fast: true,
        }
    }

    /// Mode fail-fast : arrêt au premier échec.
    pub fn fail_fast(mut self) -> Self {
        self.fail_fast = true;
        self
    }

    /// Mode relentless : continue malgré les échecs (mode Psychopathy).
    pub fn relentless(mut self) -> Self {
        self.fail_fast = false;
        self
    }

    /// Ajoute une étape au pipeline.
    pub fn add_step(&mut self, step: Box<dyn PipelineStep>) {
        self.steps.push(step);
    }

    /// Exécute toutes les étapes dans l'ordre.
    ///
    /// Pour chaque étape :
    /// 1. Exécute l'étape
    /// 2. Si échec → retry (jusqu'à max_retries)
    /// 3. Si échec après retries → rollback
    /// 4. Si fail_fast → stop, sinon continue
    pub fn execute(&mut self, ctx: &mut PipelineContext) {
        for i in 0..self.steps.len() {
            if ctx.should_stop {
                break;
            }

            let step = &mut self.steps[i];
            let step_name = String::from(step.name());
            let mut result = step.execute(ctx);

            // Retry logic
            let max_retries = step.max_retries();
            let mut attempt = 0;
            while result.status == StepStatus::Failed
                && attempt < max_retries
                && step.should_retry(&result)
            {
                attempt += 1;
                result = step.execute(ctx);
            }

            // Rollback en cas d'échec final
            if result.status == StepStatus::Failed {
                step.rollback(ctx);
            }

            ctx.results.push((step_name, result));

            // Vérifier fail_fast
            if self.fail_fast {
                if let Some(last) = ctx.last_result() {
                    if last.status == StepStatus::Failed {
                        ctx.should_stop = true;
                    }
                }
            }
        }
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Audit Trail ───────────────────────────────────────────────────────────

/// Niveau de sévérité d'une entrée d'audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditLevel {
    Debug,
    Info,
    Warn,
    Error,
    Success,
}

/// Une entrée dans le journal d'audit.
///
/// Chaque action est enregistrée de manière immutable.
/// Pattern Observer : l'AuditTrail est le "observer" qui enregistre
/// toutes les actions des agents et des outils.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Timestamp (ticks UEFI ou compteur monotone)
    pub timestamp: u64,
    /// Niveau de sévérité
    pub level: AuditLevel,
    /// Agent qui a effectué l'action
    pub agent: String,
    /// Action effectuée (nom de l'outil ou étape)
    pub action: String,
    /// Détail de l'action
    pub detail: String,
}

impl AuditEntry {
    pub fn is_error(&self) -> bool {
        self.level == AuditLevel::Error
    }

    pub fn is_success(&self) -> bool {
        self.level == AuditLevel::Success
    }
}

/// Journal d'audit immutable — enregistre toutes les opérations.
///
/// Pattern Observer : chaque agent/outil notifie l'AuditTrail lors
/// de ses actions. Le journal est immuable (append-only) pour
/// garantir la traçabilité.
///
/// Dans TDT, l'audit trail est "immutable, encrypted" — ici on se
/// limite à l'immutabilité (pas de chiffrement en UEFI).
pub struct AuditTrail {
    entries: Vec<AuditEntry>,
    counter: u64,
}

impl AuditTrail {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            counter: 0,
        }
    }

    /// Enregistre une nouvelle entrée.
    pub fn record(&mut self, level: AuditLevel, agent: &str, action: &str, detail: &str) {
        self.counter += 1;
        self.entries.push(AuditEntry {
            timestamp: self.counter,
            level,
            agent: String::from(agent),
            action: String::from(action),
            detail: String::from(detail),
        });
    }

    /// Nombre total d'entrées.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Vrai si le journal est vide.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Filtre les entrées par niveau.
    pub fn filter_by_level(&self, level: AuditLevel) -> Vec<&AuditEntry> {
        self.entries.iter().filter(|e| e.level == level).collect()
    }

    /// Filtre les entrées par agent.
    pub fn filter_by_agent(&self, agent: &str) -> Vec<&AuditEntry> {
        self.entries.iter().filter(|e| e.agent == agent).collect()
    }

    /// Compte le nombre d'erreurs.
    pub fn error_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_error()).count()
    }

    /// Compte le nombre de succès.
    pub fn success_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_success()).count()
    }

    /// Vérifie si tout s'est bien passé.
    pub fn all_success(&self) -> bool {
        self.error_count() == 0
    }

    /// Itère sur toutes les entrées.
    pub fn iter(&self) -> impl Iterator<Item = &AuditEntry> {
        self.entries.iter()
    }

    /// Vide le journal.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.counter = 0;
    }
}

impl Default for AuditTrail {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Strategy ──────────────────────────────────────────────────────────────

/// Stratégie de spoofing — détermine COMMENT les valeurs sont générées.
///
/// Inspiré du pattern Strategy de la POO, mais avec une dimension
/// "personnalité" issue de TDT. Chaque stratégie correspond à un
/// mode d'attaque différent.
///
/// C'est l'équivalent du `PersonalitySelector` de TDT.
pub trait SpoofStrategy {
    /// Nom de la stratégie.
    fn name(&self) -> &str;

    /// Mode de personnalité associé.
    fn mode(&self) -> PersonalityMode;

    /// Doit-on spoof ce champ ?
    /// Ex: Machiavellianism peut choisir de ne PAS spoof
    /// un champ si l'original est déjà un placeholder.
    fn should_spoof_field(&self, field_name: &str, original_value: &str) -> bool;

    /// Niveau d'agressivité (0-100) : combien de changements appliquer.
    fn aggression_level(&self) -> u8;

    /// Niveau de furtivité (0-100) : à quel point préserver le format.
    fn stealth_level(&self) -> u8;

    /// Type de générateur à utiliser pour les UUIDs.
    fn uuid_mode(&self) -> UuidGenerationMode;

    /// Type de générateur pour les numéros de série.
    fn serial_mode(&self) -> SerialGenerationMode;
}

/// Mode de génération d'UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UuidGenerationMode {
    /// UUID v4 standard (RFC 4122)
    Standard,
    /// UUID complètement aléatoire (16 bytes random)
    Random,
    /// UUID basé sur l'original (quelques bits modifiés)
    SimilarToOriginal,
}

/// Mode de génération de numéros de série.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialGenerationMode {
    /// Même format que l'original (longueur, classes de caractères)
    MatchFormat,
    /// Format basé sur le constructeur détecté (Dell, HP, Lenovo...)
    VendorAware,
    /// Aléatoire pur, ignore le format original
    Random,
    /// Préserve une partie de l'original (furtif)
    PartialPreserve,
}

// ─── Implémentations concrètes des stratégies ──────────────────────────────

/// Stratégie agressive — Narcissism.
///
/// Change TOUT de manière maximale. UUID aléatoire, séries longues,
/// aucun respect du format original. Rapide mais facilement détectable.
pub struct AggressiveStrategy;

impl SpoofStrategy for AggressiveStrategy {
    fn name(&self) -> &str { "Aggressive (Narcissism)" }

    fn mode(&self) -> PersonalityMode { PersonalityMode::Narcissism }

    fn should_spoof_field(&self, _field_name: &str, _original_value: &str) -> bool {
        true // Spoof TOUT, toujours
    }

    fn aggression_level(&self) -> u8 { 100 }
    fn stealth_level(&self) -> u8 { 5 }

    fn uuid_mode(&self) -> UuidGenerationMode {
        UuidGenerationMode::Random // Le plus radical
    }

    fn serial_mode(&self) -> SerialGenerationMode {
        SerialGenerationMode::Random // Ignore le format original
    }
}

/// Stratégie furtive — Machiavellianism.
///
/// Change le minimum nécessaire. Préserve les formats, n'agit que si
/// l'original est une valeur réelle (pas un placeholder).
/// Indétectable mais moins complet.
pub struct StealthStrategy;

impl SpoofStrategy for StealthStrategy {
    fn name(&self) -> &str { "Stealth (Machiavellianism)" }

    fn mode(&self) -> PersonalityMode { PersonalityMode::Machiavellianism }

    fn should_spoof_field(&self, _field_name: &str, original_value: &str) -> bool {
        // Ne spoof que si la valeur originale n'est pas un placeholder
        let lower = original_value.to_lowercase();
        !lower.contains("default string")
            && !lower.contains("to be filled")
            && !lower.contains("not specified")
            && !lower.contains("unknown")
            && !lower.is_empty()
    }

    fn aggression_level(&self) -> u8 { 20 }
    fn stealth_level(&self) -> u8 { 100 }

    fn uuid_mode(&self) -> UuidGenerationMode {
        UuidGenerationMode::SimilarToOriginal // Changement minimal
    }

    fn serial_mode(&self) -> SerialGenerationMode {
        SerialGenerationMode::VendorAware // Crédible pour le constructeur
    }
}

/// Stratégie relentless — Psychopathy.
///
/// Essaie TOUS les patchs. Retry infini. Ignore les erreurs.
/// Combine les approches. Brutal mais efficace.
pub struct RelentlessStrategy;

impl SpoofStrategy for RelentlessStrategy {
    fn name(&self) -> &str { "Relentless (Psychopathy)" }

    fn mode(&self) -> PersonalityMode { PersonalityMode::Psychopathy }

    fn should_spoof_field(&self, _field_name: &str, _original_value: &str) -> bool {
        true // Spoof même les placeholders, on s'en fout
    }

    fn aggression_level(&self) -> u8 { 85 }
    fn stealth_level(&self) -> u8 { 0 }

    fn uuid_mode(&self) -> UuidGenerationMode {
        UuidGenerationMode::Random
    }

    fn serial_mode(&self) -> SerialGenerationMode {
        SerialGenerationMode::Random
    }
}

/// Stratégie équilibrée — Fusion Engine TDT.
///
/// Mixe les approches des trois personnalités selon le contexte.
/// Par défaut : format-aware (Mach), mais n'hésite pas à être
/// agressif si nécessaire (Narc), avec retry (Psycho).
pub struct BalancedStrategy;

impl SpoofStrategy for BalancedStrategy {
    fn name(&self) -> &str { "Balanced (Fusion)" }

    fn mode(&self) -> PersonalityMode { PersonalityMode::Balanced }

    fn should_spoof_field(&self, field_name: &str, original_value: &str) -> bool {
        // Comportement adaptatif : agressif sur UUID, furtif sur les séries
        if field_name.contains("uuid") {
            return true; // Toujours spoof l'UUID
        }
        let lower = original_value.to_lowercase();
        !lower.contains("default string")
            && !lower.contains("to be filled")
            && !lower.is_empty()
    }

    fn aggression_level(&self) -> u8 { 50 }
    fn stealth_level(&self) -> u8 { 60 }

    fn uuid_mode(&self) -> UuidGenerationMode {
        UuidGenerationMode::Standard // UUID v4 standard
    }

    fn serial_mode(&self) -> SerialGenerationMode {
        SerialGenerationMode::VendorAware
    }
}

/// Factory de stratégies — crée la stratégie appropriée.
///
/// Inspiré du `PersonalitySelector` de TDT.
pub struct StrategyFactory;

impl StrategyFactory {
    /// Crée une stratégie pour un mode donné.
    pub fn create(mode: PersonalityMode) -> Box<dyn SpoofStrategy> {
        match mode {
            PersonalityMode::Narcissism => Box::new(AggressiveStrategy),
            PersonalityMode::Psychopathy => Box::new(RelentlessStrategy),
            PersonalityMode::Machiavellianism => Box::new(StealthStrategy),
            PersonalityMode::Balanced => Box::new(BalancedStrategy),
        }
    }

    /// Crée une stratégie fusion (mélange de deux personnalités).
    /// `primary` = personnalité dominante, `secondary` = influence secondaire.
    /// `primary_weight` = poids de la personnalité primaire (0-100).
    ///
    /// Exemples TDT :
    /// - "Patient Predator" → Mach 80% + Psych 20%
    /// - "Cocky Assassin" → Narc 60% + Mach 40%
    /// - "Berserker" → Psych 70% + Narc 30%
    /// - "Ghost" → Mach 90% + Psych 10%
    pub fn create_fusion(
        primary: PersonalityMode,
        secondary: PersonalityMode,
        primary_weight: u8,
    ) -> Box<dyn SpoofStrategy> {
        let name = FusionStrategy::build_name(primary, secondary, primary_weight);
        Box::new(FusionStrategy {
            primary,
            secondary,
            primary_weight,
            cached_name: name,
        })
    }

    /// Liste toutes les stratégies disponibles.
    pub fn list_all() -> Vec<Box<dyn SpoofStrategy>> {
        vec![
            Box::new(AggressiveStrategy),
            Box::new(StealthStrategy),
            Box::new(RelentlessStrategy),
            Box::new(BalancedStrategy),
        ]
    }
}

/// Stratégie de fusion — combine deux personnalités.
///
/// Les paramètres sont interpolés entre les deux personnalités
/// selon le poids défini. C'est le "Fusion Engine" de TDT.
struct FusionStrategy {
    primary: PersonalityMode,
    secondary: PersonalityMode,
    primary_weight: u8, // 0-100
    cached_name: String,
}

impl FusionStrategy {
    fn build_name(primary: PersonalityMode, secondary: PersonalityMode, weight: u8) -> String {
        let p_name = match primary {
            PersonalityMode::Narcissism => "Narc",
            PersonalityMode::Psychopathy => "Psych",
            PersonalityMode::Machiavellianism => "Mach",
            PersonalityMode::Balanced => "Bal",
        };
        let s_name = match secondary {
            PersonalityMode::Narcissism => "Narc",
            PersonalityMode::Psychopathy => "Psych",
            PersonalityMode::Machiavellianism => "Mach",
            PersonalityMode::Balanced => "Bal",
        };
        alloc::format!("Fusion({}%{} + {}%{})", weight, p_name, 100 - weight, s_name)
    }
}

impl SpoofStrategy for FusionStrategy {
    fn name(&self) -> &str {
        &self.cached_name
    }

    fn mode(&self) -> PersonalityMode {
        if self.primary_weight >= 50 {
            self.primary
        } else {
            self.secondary
        }
    }

    fn should_spoof_field(&self, field_name: &str, original_value: &str) -> bool {
        // Délégue à la stratégie primaire ou secondaire selon le poids
        let primary_strat = StrategyFactory::create(self.primary);
        let secondary_strat = StrategyFactory::create(self.secondary);

        let primary_decision = primary_strat.should_spoof_field(field_name, original_value);
        let secondary_decision = secondary_strat.should_spoof_field(field_name, original_value);

        // Si les deux sont d'accord, on suit
        if primary_decision == secondary_decision {
            return primary_decision;
        }

        // Si divergence → la primaire l'emporte proportionnellement au poids
        // Ex: Mach 80% + Psych 20% → 80% de chance de suivre Mach
        self.primary_weight >= 50
    }

    fn aggression_level(&self) -> u8 {
        let p = self.primary.aggression() as u16;
        let s = self.secondary.aggression() as u16;
        let w = self.primary_weight as u16;
        ((p * w + s * (100 - w)) / 100) as u8
    }

    fn stealth_level(&self) -> u8 {
        let p = self.primary.stealth() as u16;
        let s = self.secondary.stealth() as u16;
        let w = self.primary_weight as u16;
        ((p * w + s * (100 - w)) / 100) as u8
    }

    fn uuid_mode(&self) -> UuidGenerationMode {
        if self.primary_weight >= 50 {
            StrategyFactory::create(self.primary).uuid_mode()
        } else {
            StrategyFactory::create(self.secondary).uuid_mode()
        }
    }

    fn serial_mode(&self) -> SerialGenerationMode {
        if self.primary_weight >= 50 {
            StrategyFactory::create(self.primary).serial_mode()
        } else {
            StrategyFactory::create(self.secondary).serial_mode()
        }
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── PersonalityMode ────────────────────────────────────────────────────

    #[test]
    fn test_personality_aggression_range() {
        for mode in &[
            PersonalityMode::Narcissism,
            PersonalityMode::Psychopathy,
            PersonalityMode::Machiavellianism,
            PersonalityMode::Balanced,
        ] {
            let agg = mode.aggression();
            assert!(agg <= 100, "Aggression should be <= 100 for {:?}", mode);
        }
        assert_eq!(PersonalityMode::Narcissism.aggression(), 100);
        assert_eq!(PersonalityMode::Machiavellianism.aggression(), 20);
    }

    #[test]
    fn test_personality_stealth_range() {
        assert!(PersonalityMode::Narcissism.stealth() < 10);
        assert_eq!(PersonalityMode::Machiavellianism.stealth(), 100);
        assert_eq!(PersonalityMode::Psychopathy.stealth(), 0);
    }

    #[test]
    fn test_personality_max_steps_order() {
        // Psychopathy doit avoir le plus de steps (persistence infinie)
        let psycho = PersonalityMode::Psychopathy.default_max_steps();
        let mach = PersonalityMode::Machiavellianism.default_max_steps();
        let narc = PersonalityMode::Narcissism.default_max_steps();
        assert!(psycho > mach);
        assert!(mach > narc);
    }

    // ── AgentContext ───────────────────────────────────────────────────────

    #[test]
    fn test_agent_context_budgets() {
        let mut ctx = AgentContext::new();
        assert!(!ctx.step_budget_exceeded(5));
        assert!(!ctx.error_budget_exceeded(3));

        ctx.step_count = 6;
        assert!(ctx.step_budget_exceeded(5));

        ctx.record_error("test error");
        ctx.record_error("test error 2");
        ctx.record_error("test error 3");
        assert!(ctx.error_budget_exceeded(3));
    }

    #[test]
    fn test_agent_context_confidence_decay() {
        let mut ctx = AgentContext::new();
        ctx.confidence = 1.0;
        ctx.record_error("err");
        assert!(ctx.confidence < 0.9, "Confidence should decay on error");
    }

    // ── AgentData ──────────────────────────────────────────────────────────

    #[test]
    fn test_agent_data_set_get() {
        let mut data = AgentData::default();
        data.set("key1", "value1");
        assert_eq!(data.get("key1"), Some("value1"));
        assert_eq!(data.get("nonexistent"), None);
    }

    #[test]
    fn test_agent_data_overwrite() {
        let mut data = AgentData::default();
        data.set("key", "old");
        data.set("key", "new");
        assert_eq!(data.get("key"), Some("new"));
    }

    // ── ToolRegistry ───────────────────────────────────────────────────────

    struct DummyTool {
        name: &'static str,
        desc: &'static str,
        destructive: bool,
    }

    impl Tool for DummyTool {
        fn name(&self) -> &str { self.name }
        fn description(&self) -> &str { self.desc }
        fn execute(&mut self, _params: &ToolParams) -> ToolResult {
            ToolResult::Success {
                message: String::from("ok"),
                data: None,
            }
        }
        fn is_destructive(&self) -> bool { self.destructive }
    }

    #[test]
    fn test_tool_registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool {
            name: "test_tool",
            desc: "A test tool",
            destructive: false,
        }));
        assert_eq!(registry.len(), 1);
        assert!(registry.get("test_tool").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_tool_registry_machiavellianism_filters_destructive() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool {
            name: "safe_tool",
            desc: "safe",
            destructive: false,
        }));
        registry.register(Box::new(DummyTool {
            name: "dangerous_tool",
            desc: "danger",
            destructive: true,
        }));

        let mach_tools = registry.list_for_personality(PersonalityMode::Machiavellianism);
        assert_eq!(mach_tools.len(), 1);
        assert_eq!(mach_tools[0].name(), "safe_tool");

        let psycho_tools = registry.list_for_personality(PersonalityMode::Psychopathy);
        assert_eq!(psycho_tools.len(), 2); // Psychopathy prend tout
    }

    #[test]
    fn test_tool_registry_narcissism_prioritizes_destructive() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool {
            name: "safe",
            desc: "s",
            destructive: false,
        }));
        registry.register(Box::new(DummyTool {
            name: "danger",
            desc: "d",
            destructive: true,
        }));

        let narc_tools = registry.list_for_personality(PersonalityMode::Narcissism);
        assert_eq!(narc_tools.len(), 2);
        // L'outil destructif doit être en premier (priorité 0)
        assert_eq!(narc_tools[0].name(), "danger");
    }

    // ── ToolParams ─────────────────────────────────────────────────────────

    #[test]
    fn test_tool_params() {
        let params = ToolParams::new()
            .with("field", "uuid")
            .with("mode", "random");
        assert_eq!(params.get("field"), Some("uuid"));
        assert_eq!(params.get("mode"), Some("random"));
        assert!(params.has("field"));
        assert!(!params.has("nonexistent"));
    }

    // ── AuditTrail ─────────────────────────────────────────────────────────

    #[test]
    fn test_audit_trail_basic() {
        let mut audit = AuditTrail::new();
        assert!(audit.is_empty());

        audit.record(AuditLevel::Info, "test_agent", "action1", "detail");
        assert_eq!(audit.len(), 1);
        assert_eq!(audit.error_count(), 0);
        assert_eq!(audit.success_count(), 0);

        audit.record(AuditLevel::Error, "test_agent", "action2", "failed");
        assert_eq!(audit.len(), 2);
        assert_eq!(audit.error_count(), 1);

        audit.record(AuditLevel::Success, "agent2", "action3", "ok");
        assert_eq!(audit.success_count(), 1);
    }

    #[test]
    fn test_audit_trail_filter_by_agent() {
        let mut audit = AuditTrail::new();
        audit.record(AuditLevel::Info, "agent_a", "x", "d");
        audit.record(AuditLevel::Info, "agent_b", "y", "d");
        audit.record(AuditLevel::Info, "agent_a", "z", "d");

        let a_entries = audit.filter_by_agent("agent_a");
        assert_eq!(a_entries.len(), 2);
    }

    #[test]
    fn test_audit_trail_all_success() {
        let mut audit = AuditTrail::new();
        audit.record(AuditLevel::Info, "a", "x", "d");
        audit.record(AuditLevel::Success, "a", "y", "d");
        assert!(audit.all_success());

        audit.record(AuditLevel::Error, "a", "z", "fail");
        assert!(!audit.all_success());
    }

    // ── Pipeline ───────────────────────────────────────────────────────────

    struct TestStep {
        name: &'static str,
        succeed: bool,
        executed: bool,
    }

    impl PipelineStep for TestStep {
        fn name(&self) -> &str { self.name }
        fn execute(&mut self, _ctx: &mut PipelineContext) -> StepResult {
            self.executed = true;
            if self.succeed {
                StepResult::ok("ok", 1)
            } else {
                StepResult::fail("fail", 1)
            }
        }
    }

    #[test]
    fn test_pipeline_fail_fast() {
        let mut pipeline = Pipeline::new().fail_fast();
        pipeline.add_step(Box::new(TestStep { name: "s1", succeed: true, executed: false }));
        pipeline.add_step(Box::new(TestStep { name: "s2", succeed: false, executed: false }));
        pipeline.add_step(Box::new(TestStep { name: "s3", succeed: true, executed: false }));

        let mut ctx = PipelineContext::new();
        pipeline.execute(&mut ctx);

        assert_eq!(ctx.results.len(), 2); // s3 pas exécuté (fail_fast)
        assert!(ctx.results[0].1.status == StepStatus::Ok);
        assert!(ctx.results[1].1.status == StepStatus::Failed);
        assert!(ctx.should_stop);
    }

    #[test]
    fn test_pipeline_relentless() {
        let mut pipeline = Pipeline::new().relentless();
        pipeline.add_step(Box::new(TestStep { name: "s1", succeed: true, executed: false }));
        pipeline.add_step(Box::new(TestStep { name: "s2", succeed: false, executed: false }));
        pipeline.add_step(Box::new(TestStep { name: "s3", succeed: true, executed: false }));

        let mut ctx = PipelineContext::new();
        pipeline.execute(&mut ctx);

        assert_eq!(ctx.results.len(), 3); // Tout exécuté (relentless)
    }

    // ── Strategy ───────────────────────────────────────────────────────────

    #[test]
    fn test_aggressive_strategy_spoofs_everything() {
        let strat = AggressiveStrategy;
        assert!(strat.should_spoof_field("uuid", ""));
        assert!(strat.should_spoof_field("serial", "Default string"));
        assert_eq!(strat.aggression_level(), 100);
        assert!(strat.stealth_level() < 10);
    }

    #[test]
    fn test_stealth_strategy_skips_placeholders() {
        let strat = StealthStrategy;
        assert!(!strat.should_spoof_field("serial", "Default string"));
        assert!(!strat.should_spoof_field("serial", "To Be Filled By O.E.M."));
        assert!(!strat.should_spoof_field("serial", ""));
        assert!(strat.should_spoof_field("uuid", "12345678"));
        assert_eq!(strat.stealth_level(), 100);
    }

    #[test]
    fn test_strategy_factory_creates_correct_types() {
        let aggressive = StrategyFactory::create(PersonalityMode::Narcissism);
        assert_eq!(aggressive.mode(), PersonalityMode::Narcissism);
        assert_eq!(aggressive.aggression_level(), 100);

        let stealth = StrategyFactory::create(PersonalityMode::Machiavellianism);
        assert_eq!(stealth.mode(), PersonalityMode::Machiavellianism);
        assert_eq!(stealth.stealth_level(), 100);

        let relentless = StrategyFactory::create(PersonalityMode::Psychopathy);
        assert_eq!(relentless.mode(), PersonalityMode::Psychopathy);
        assert_eq!(relentless.stealth_level(), 0);
    }

    #[test]
    fn test_fusion_strategy_weights() {
        // Patient Predator : Mach 80% + Psych 20%
        let fusion = StrategyFactory::create_fusion(
            PersonalityMode::Machiavellianism,
            PersonalityMode::Psychopathy,
            80,
        );
        // Doit pencher vers Machiavellianism (furtif)
        assert!(fusion.stealth_level() > 50);
        assert!(fusion.aggression_level() < 80);
    }

    #[test]
    fn test_strategy_list_all() {
        let all = StrategyFactory::list_all();
        assert_eq!(all.len(), 4);
    }

    // ── ToolResult ─────────────────────────────────────────────────────────

    #[test]
    fn test_tool_result_predicates() {
        assert!(ToolResult::Success {
            message: String::from("ok"),
            data: None,
        }
        .is_success());

        assert!(ToolResult::FatalError {
            message: String::from("dead"),
        }
        .is_fatal());

        assert!(!ToolResult::RetryableError {
            message: String::from("temp"),
            retry_after_ms: 100,
        }
        .is_fatal());

        assert!(ToolResult::RetryableError {
            message: String::from("temp"),
            retry_after_ms: 100,
        }
        .is_error());
    }
}
