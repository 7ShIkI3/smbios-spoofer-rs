//! # Memory Manager — Mémoire Persistante et Contextuelle
//!
//! Inspiré de :
//! - **Hermes Agent** (NousResearch) : `memory_manager.py` → MEMORY.md + USER.md
//! - **OpenClaw** : FractalMemory (SQLite FTS5 + LanceDB embeddings), heartbeat distillation
//! - **Claude Code** : `~/.claude/projects/<hash>/memory/` — facts persist across sessions
//! - **Agent Zero** : FAISS-based memory avec Fragments/Solutions/Metadata
//! - **MemGPT/Letta** : stateful agents with memory, reasoning, and context management
//!
//! ## Architecture mémoire
//!
//! ```
//! ┌─────────────────────────────────────────────────────┐
//! │              MemoryManager                          │
//! ├─────────────┬──────────────┬────────────────────────┤
//! │ Working     │ Episodic     │ Semantic Memory        │
//! │ Memory      │ Memory       │ (long-term facts)      │
//! │ (session)   │ (past runs)  │                        │
//! ├─────────────┼──────────────┼────────────────────────┤
//! │ Vec<String> │ Vec<Episode> │ Vec<SemanticFact>      │
//! │ volatile    │ NVRAM/disk   │ NVRAM/disk             │
//! └─────────────┴──────────────┴────────────────────────┘
//! ```
//!
//! ## Types de mémoire
//!
//! | Type           | Persistance | Description                              |
//! |----------------|-------------|------------------------------------------|
//! | **Working**    | Volatile    | Contexte de la session courante           |
//! | **Episodic**   | Persistant  | Historique des runs précédents           |
//! | **Semantic**   | Persistant  | Faits appris, patterns, préférences       |
//! | **Procedural** | Persistant  | Stratégies qui ont marché, skills        |

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;

// ─── Working Memory ─────────────────────────────────────────────────────────

/// Mémoire de travail — volatile, effacée à chaque reboot.
///
/// Stocke les observations, décisions et résultats de la session courante.
/// Équivalent du `AgentContext.observations` mais structuré par catégorie.
pub struct WorkingMemory {
    /// Observations brutes (ce que l'agent a vu)
    observations: Vec<String>,
    /// Décisions prises (outil choisi, pourquoi)
    decisions: Vec<DecisionRecord>,
    /// Résultats obtenus
    outcomes: Vec<OutcomeRecord>,
    /// Messages d'erreur non-fatals
    errors: Vec<String>,
    /// Prochaine action prévue
    next_action: Option<String>,
    /// Priorités courantes
    priorities: Vec<String>,
}

/// Une décision enregistrée.
#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub step: usize,
    pub tool_name: String,
    pub reasoning: String,
    pub timestamp: u64,
}

/// Un résultat enregistré.
#[derive(Debug, Clone)]
pub struct OutcomeRecord {
    pub step: usize,
    pub success: bool,
    pub message: String,
    pub duration_ms: u64,
}

impl WorkingMemory {
    pub fn new() -> Self {
        Self {
            observations: Vec::new(),
            decisions: Vec::new(),
            outcomes: Vec::new(),
            errors: Vec::new(),
            next_action: None,
            priorities: Vec::new(),
        }
    }

    pub fn observe(&mut self, msg: &str) {
        self.observations.push(String::from(msg));
    }

    pub fn decide(&mut self, step: usize, tool: &str, reasoning: &str) {
        self.decisions.push(DecisionRecord {
            step,
            tool_name: String::from(tool),
            reasoning: String::from(reasoning),
            timestamp: step as u64,
        });
    }

    pub fn record_outcome(&mut self, step: usize, success: bool, message: &str, duration_ms: u64) {
        self.outcomes.push(OutcomeRecord {
            step,
            success,
            message: String::from(message),
            duration_ms,
        });
    }

    pub fn record_error(&mut self, error: &str) {
        self.errors.push(String::from(error));
    }

    pub fn set_next_action(&mut self, action: &str) {
        self.next_action = Some(String::from(action));
    }

    /// Résumé texte de la session courante.
    pub fn summarize(&self) -> String {
        let total = self.outcomes.len();
        let successes = self.outcomes.iter().filter(|o| o.success).count();
        format!(
            "Session: {} obs, {} decisions, {}/{} success, {} errors",
            self.observations.len(),
            self.decisions.len(),
            successes,
            total,
            self.errors.len()
        )
    }

    /// Dernière observation.
    pub fn last_observation(&self) -> Option<&str> {
        self.observations.last().map(|s| s.as_str())
    }

    /// Dernière décision.
    pub fn last_decision(&self) -> Option<&DecisionRecord> {
        self.decisions.last()
    }

    /// Taux de succès (0.0 - 1.0).
    pub fn success_rate(&self) -> f32 {
        if self.outcomes.is_empty() {
            return 1.0;
        }
        let successes = self.outcomes.iter().filter(|o| o.success).count();
        successes as f32 / self.outcomes.len() as f32
    }

    /// Efface tout (nouveau cycle).
    pub fn clear(&mut self) {
        self.observations.clear();
        self.decisions.clear();
        self.outcomes.clear();
        self.errors.clear();
        self.next_action = None;
        self.priorities.clear();
    }

    /// Nombre total d'entrées.
    pub fn len(&self) -> usize {
        self.observations.len() + self.decisions.len() + self.outcomes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for WorkingMemory {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Episodic Memory ────────────────────────────────────────────────────────

/// Un épisode — une exécution complète du spoofer.
///
/// Persisté en NVRAM/disk entre les boots.
#[derive(Debug, Clone)]
pub struct Episode {
    /// Identifiant unique
    pub id: u32,
    /// Timestamp UEFI
    pub timestamp: u64,
    /// UUID utilisé
    pub uuid: [u8; 16],
    /// Taux de succès global
    pub success_rate: f32,
    /// Nombre d'étapes
    pub steps: usize,
    /// Nombre d'erreurs
    pub errors: usize,
    /// Stratégie utilisée
    pub strategy: String,
    /// Résumé texte
    pub summary: String,
}

impl Episode {
    pub fn new(id: u32, uuid: &[u8; 16], strategy: &str) -> Self {
        Self {
            id,
            timestamp: 0,
            uuid: *uuid,
            success_rate: 0.0,
            steps: 0,
            errors: 0,
            strategy: String::from(strategy),
            summary: String::new(),
        }
    }
}

/// Mémoire épisodique — historique des runs précédents.
///
/// Pattern : Hermes `sessions/`, OpenClaw `session_search`.
/// Permet de savoir ce qui a marché ou échoué dans le passé.
pub struct EpisodicMemory {
    episodes: Vec<Episode>,
    max_episodes: usize,
    next_id: u32,
}

impl EpisodicMemory {
    pub fn new(max_episodes: usize) -> Self {
        Self {
            episodes: Vec::new(),
            max_episodes,
            next_id: 1,
        }
    }

    /// Ajoute un nouvel épisode.
    pub fn add(&mut self, uuid: &[u8; 16], strategy: &str) -> &Episode {
        let episode = Episode::new(self.next_id, uuid, strategy);
        self.next_id += 1;
        self.episodes.push(episode);
        // Garder sous la limite
        while self.episodes.len() > self.max_episodes {
            self.episodes.remove(0);
        }
        self.episodes.last().unwrap()
    }

    /// Met à jour le dernier épisode avec les stats finales.
    pub fn finalize_last(
        &mut self,
        steps: usize,
        errors: usize,
        success_rate: f32,
        summary: &str,
    ) {
        if let Some(ep) = self.episodes.last_mut() {
            ep.steps = steps;
            ep.errors = errors;
            ep.success_rate = success_rate;
            ep.summary = String::from(summary);
        }
    }

    /// Meilleur épisode (par success_rate).
    pub fn best_episode(&self) -> Option<&Episode> {
        self.episodes
            .iter()
            .max_by(|a, b| a.success_rate.partial_cmp(&b.success_rate).unwrap_or(core::cmp::Ordering::Equal))
    }

    /// Pire épisode.
    pub fn worst_episode(&self) -> Option<&Episode> {
        self.episodes
            .iter()
            .min_by(|a, b| a.success_rate.partial_cmp(&b.success_rate).unwrap_or(core::cmp::Ordering::Equal))
    }

    /// Taux de succès moyen sur tous les épisodes.
    pub fn average_success_rate(&self) -> f32 {
        if self.episodes.is_empty() {
            return 0.0;
        }
        self.episodes.iter().map(|e| e.success_rate).sum::<f32>()
            / self.episodes.len() as f32
    }

    /// Stratégie qui a le meilleur taux de succès.
    pub fn best_strategy(&self) -> Option<&str> {
        let mut strategy_scores: BTreeMap<&str, (f32, usize)> = BTreeMap::new();
        for ep in &self.episodes {
            let entry = strategy_scores
                .entry(ep.strategy.as_str())
                .or_insert((0.0, 0));
            entry.0 += ep.success_rate;
            entry.1 += 1;
        }
        strategy_scores
            .iter()
            .max_by(|(_, (sa, ca)), (_, (sb, cb))| {
                let a_avg = sa / *ca as f32;
                let b_avg = sb / *cb as f32;
                a_avg.partial_cmp(&b_avg).unwrap_or(core::cmp::Ordering::Equal)
            })
            .map(|(s, _)| *s)
    }

    /// Nombre d'épisodes stockés.
    pub fn len(&self) -> usize {
        self.episodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty()
    }

    /// Tendance d'amélioration.
    pub fn trend(&self) -> f32 {
        if self.episodes.len() < 2 {
            return 0.0;
        }
        let half = self.episodes.len() / 2;
        let first: f32 = self.episodes[..half].iter().map(|e| e.success_rate).sum::<f32>() / half as f32;
        let second: f32 = self.episodes[half..].iter().map(|e| e.success_rate).sum::<f32>() / (self.episodes.len() - half) as f32;
        second - first
    }

    /// L'agent s'améliore-t-il au fil du temps ?
    pub fn is_improving(&self) -> bool {
        self.trend() > 0.05
    }
}

// ─── Semantic Memory ────────────────────────────────────────────────────────

/// Un fait sémantique — connaissance persistante.
///
/// Pattern : Claude Code memory files, OpenClaw MEMORY.md, Hermes memory system.
#[derive(Debug, Clone)]
pub struct SemanticFact {
    /// Catégorie (ex: "vendor", "firmware", "error", "strategy")
    pub category: String,
    /// Clé unique
    pub key: String,
    /// Valeur
    pub value: String,
    /// Confiance dans ce fait (0.0 - 1.0)
    pub confidence: f32,
    /// Nombre de fois que ce fait a été vérifié
    pub verification_count: usize,
}

impl SemanticFact {
    pub fn new(category: &str, key: &str, value: &str) -> Self {
        Self {
            category: String::from(category),
            key: String::from(key),
            value: String::from(value),
            confidence: 0.5,
            verification_count: 1,
        }
    }

    /// Renforce la confiance dans ce fait.
    pub fn reinforce(&mut self) {
        self.verification_count += 1;
        self.confidence = (self.confidence + 0.1).min(1.0);
    }

    /// Diminue la confiance.
    pub fn weaken(&mut self) {
        self.confidence = (self.confidence - 0.2).max(0.0);
    }
}

/// Mémoire sémantique — base de connaissances persistante.
///
/// Stocke les patterns appris, les préférences, et les faits sur
/// l'environnement (constructeur, firmware, erreurs connues).
pub struct SemanticMemory {
    facts: Vec<SemanticFact>,
}

impl SemanticMemory {
    pub fn new() -> Self {
        Self { facts: Vec::new() }
    }

    /// Ajoute ou met à jour un fait.
    pub fn set(&mut self, category: &str, key: &str, value: &str) {
        // Chercher si le fait existe déjà
        for fact in self.facts.iter_mut() {
            if fact.category == category && fact.key == key {
                fact.value = String::from(value);
                fact.reinforce();
                return;
            }
        }
        // Nouveau fait
        self.facts.push(SemanticFact::new(category, key, value));
    }

    /// Récupère un fait par catégorie et clé.
    pub fn get(&self, category: &str, key: &str) -> Option<&str> {
        self.facts
            .iter()
            .find(|f| f.category == category && f.key == key)
            .map(|f| f.value.as_str())
    }

    /// Récupère tous les faits d'une catégorie.
    pub fn get_by_category(&self, category: &str) -> Vec<&SemanticFact> {
        self.facts
            .iter()
            .filter(|f| f.category == category)
            .collect()
    }

    /// Recherche par mot-clé dans les valeurs.
    pub fn search(&self, query: &str) -> Vec<&SemanticFact> {
        let lower = query.to_lowercase();
        self.facts
            .iter()
            .filter(|f| {
                f.key.to_lowercase().contains(&lower)
                    || f.value.to_lowercase().contains(&lower)
                    || f.category.to_lowercase().contains(&lower)
            })
            .collect()
    }

    /// Faits avec une confiance élevée.
    pub fn high_confidence_facts(&self, min_confidence: f32) -> Vec<&SemanticFact> {
        self.facts
            .iter()
            .filter(|f| f.confidence >= min_confidence)
            .collect()
    }

    /// Supprime les faits de faible confiance.
    pub fn prune_low_confidence(&mut self, threshold: f32) {
        self.facts.retain(|f| f.confidence >= threshold);
    }

    pub fn len(&self) -> usize {
        self.facts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// Résumé texte de la base de connaissances.
    pub fn summary(&self) -> String {
        let mut cats: BTreeMap<&str, usize> = BTreeMap::new();
        for fact in &self.facts {
            *cats.entry(fact.category.as_str()).or_insert(0) += 1;
        }
        let parts: Vec<String> = cats
            .iter()
            .map(|(cat, count)| format!("{}: {} facts", cat, count))
            .collect();
        parts.join(", ")
    }

    /// Exporte tous les faits en texte.
    pub fn export(&self) -> String {
        let mut lines = Vec::new();
        for fact in &self.facts {
            lines.push(format!(
                "[{}] {} = {} (conf={:.1}, verified={})",
                fact.category, fact.key, fact.value,
                fact.confidence, fact.verification_count
            ));
        }
        lines.join("\n")
    }
}

impl Default for SemanticMemory {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Memory Manager ─────────────────────────────────────────────────────────

/// Gestionnaire de mémoire unifié.
///
/// Combine les trois types de mémoire et orchestre :
/// - La rétention entre sessions
/// - La consolidation des apprentissages
/// - L'oubli des informations obsolètes
///
/// Pattern : MemGPT/Letta "stateful agents", Hermes memory_manager.py.
pub struct MemoryManager {
    pub working: WorkingMemory,
    pub episodic: EpisodicMemory,
    pub semantic: SemanticMemory,
}

impl MemoryManager {
    pub fn new(max_episodes: usize) -> Self {
        Self {
            working: WorkingMemory::new(),
            episodic: EpisodicMemory::new(max_episodes),
            semantic: SemanticMemory::new(),
        }
    }

    /// Démarre un nouvel épisode.
    pub fn start_episode(&mut self, uuid: &[u8; 16], strategy: &str) {
        self.working.clear();
        let _ = self.episodic.add(uuid, strategy);
        // Apprendre de l'épisode précédent
        if let Some(prev) = self.episodic.best_episode() {
            self.semantic.set(
                "strategy",
                "best_so_far",
                &format!("{} (score: {:.1}%)", prev.strategy, prev.success_rate * 100.0),
            );
        }
    }

    /// Finalise l'épisode courant.
    pub fn end_episode(&mut self) {
        let rate = self.working.success_rate();
        let summary = self.working.summarize();
        self.episodic.finalize_last(
            self.working.outcomes.len(),
            self.working.errors.len(),
            rate,
            &summary,
        );

        // Apprentissage : mémoriser les patterns d'erreur
        for error in &self.working.errors {
            self.semantic.set("error", "last_error", error);
        }

        // Mémoriser le succès
        if rate > 0.9 {
            self.semantic.set("outcome", "last_run", "success");
        } else if rate > 0.5 {
            self.semantic.set("outcome", "last_run", "partial");
        } else {
            self.semantic.set("outcome", "last_run", "failure");
        }
    }

    /// Consolide la mémoire — nettoie les faits faibles.
    pub fn consolidate(&mut self) {
        self.semantic.prune_low_confidence(0.2);
    }

    /// Vérifie si le système apprend (s'améliore au fil des épisodes).
    pub fn is_learning(&self) -> bool {
        self.episodic.is_improving()
    }

    /// Résumé complet de l'état mémoire.
    pub fn full_summary(&self) -> String {
        format!(
            "Working: {} | Episodic: {} episodes (avg {:.0}%) | Semantic: {}",
            self.working.summarize(),
            self.episodic.len(),
            self.episodic.average_success_rate() * 100.0,
            self.semantic.summary(),
        )
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── WorkingMemory ───────────────────────────────────────────────────

    #[test]
    fn test_working_memory_basic() {
        let mut wm = WorkingMemory::new();
        assert!(wm.is_empty());

        wm.observe("SMBIOS tables found");
        wm.decide(1, "patch_type1", "need to spoof UUID");
        wm.record_outcome(1, true, "UUID patched", 10);
        assert!(!wm.is_empty());
        assert_eq!(wm.success_rate(), 1.0);
        assert_eq!(wm.last_observation(), Some("SMBIOS tables found"));
    }

    #[test]
    fn test_working_memory_partial_success() {
        let mut wm = WorkingMemory::new();
        wm.record_outcome(1, true, "ok", 10);
        wm.record_outcome(2, false, "fail", 5);
        assert_eq!(wm.success_rate(), 0.5);
    }

    #[test]
    fn test_working_memory_clear() {
        let mut wm = WorkingMemory::new();
        wm.observe("test");
        wm.clear();
        assert!(wm.is_empty());
    }

    #[test]
    fn test_working_memory_summarize() {
        let mut wm = WorkingMemory::new();
        wm.observe("obs1");
        wm.record_outcome(1, true, "ok", 1);
        wm.record_outcome(2, true, "ok", 2);
        let summary = wm.summarize();
        assert!(summary.contains("obs"));
        assert!(summary.contains("2/2"));
    }

    // ── EpisodicMemory ──────────────────────────────────────────────────

    #[test]
    fn test_episodic_memory_add_and_finalize() {
        let mut em = EpisodicMemory::new(10);
        let uuid = [0xAAu8; 16];
        em.add(&uuid, "Aggressive");
        em.finalize_last(4, 0, 1.0, "Perfect run");
        assert_eq!(em.len(), 1);
        assert_eq!(em.best_episode().unwrap().success_rate, 1.0);
    }

    #[test]
    fn test_episodic_memory_best_strategy() {
        let mut em = EpisodicMemory::new(10);
        let uuid = [0u8; 16];

        em.add(&uuid, "Stealth");
        em.finalize_last(4, 0, 0.9, "good");

        em.add(&uuid, "Aggressive");
        em.finalize_last(4, 1, 0.6, "bad");

        em.add(&uuid, "Stealth");
        em.finalize_last(4, 0, 1.0, "perfect");

        assert_eq!(em.best_strategy(), Some("Stealth"));
    }

    #[test]
    fn test_episodic_memory_max_capacity() {
        let mut em = EpisodicMemory::new(3);
        let uuid = [0u8; 16];
        for _ in 0..5 {
            em.add(&uuid, "test");
        }
        assert_eq!(em.len(), 3, "Should keep only the last 3 episodes");
    }

    #[test]
    fn test_episodic_trend_improving() {
        let mut em = EpisodicMemory::new(10);
        let uuid = [0u8; 16];
        for rate in [0.5, 0.6, 0.7, 0.8, 0.9] {
            em.add(&uuid, "test");
            em.finalize_last(4, 0, rate, "run");
        }
        assert!(em.is_improving());
    }

    // ── SemanticMemory ──────────────────────────────────────────────────

    #[test]
    fn test_semantic_memory_set_get() {
        let mut sm = SemanticMemory::new();
        sm.set("vendor", "detected", "Dell");
        assert_eq!(sm.get("vendor", "detected"), Some("Dell"));
        assert_eq!(sm.get("vendor", "unknown"), None);
    }

    #[test]
    fn test_semantic_memory_reinforce() {
        let mut sm = SemanticMemory::new();
        sm.set("vendor", "type", "Dell");
        sm.set("vendor", "type", "Dell"); // reinforce
        sm.set("vendor", "type", "Dell"); // reinforce again
        let facts = sm.get_by_category("vendor");
        assert_eq!(facts.len(), 1);
        assert!(facts[0].confidence > 0.6, "Confidence should increase");
        assert_eq!(facts[0].verification_count, 3);
    }

    #[test]
    fn test_semantic_memory_search() {
        let mut sm = SemanticMemory::new();
        sm.set("error", "nvram", "NVRAM write failed");
        sm.set("error", "disk", "Disk full");
        sm.set("vendor", "brand", "Lenovo");

        let results = sm.search("NVRAM");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "nvram");
    }

    #[test]
    fn test_semantic_memory_prune() {
        let mut sm = SemanticMemory::new();
        sm.set("test", "low", "value");
        // Manually weaken
        if let Some(fact) = sm.facts.iter_mut().find(|f| f.key == "low") {
            fact.confidence = 0.1;
        }
        sm.set("test", "high", "value");
        if let Some(fact) = sm.facts.iter_mut().find(|f| f.key == "high") {
            fact.confidence = 0.9;
        }

        sm.prune_low_confidence(0.5);
        assert_eq!(sm.len(), 1);
        assert_eq!(sm.get("test", "high"), Some("value"));
    }

    // ── MemoryManager ───────────────────────────────────────────────────

    #[test]
    fn test_memory_manager_episode_lifecycle() {
        let mut mm = MemoryManager::new(10);
        let uuid = [0x42u8; 16];

        mm.start_episode(&uuid, "Balanced");
        mm.working.record_outcome(1, true, "step1", 5);
        mm.working.record_outcome(2, true, "step2", 3);
        mm.end_episode();

        assert_eq!(mm.episodic.len(), 1);
        assert_eq!(
            mm.semantic.get("outcome", "last_run"),
            Some("success")
        );
    }

    #[test]
    fn test_memory_manager_learning_detection() {
        let mut mm = MemoryManager::new(5);
        let uuid = [0u8; 16];

        for rate in [0.5, 0.7, 0.9] {
            mm.start_episode(&uuid, "test");
            mm.working.record_outcome(1, true, "step", 1);
            mm.end_episode();
            // Force le success rate
            if let Some(ep) = mm.episodic.episodes.last_mut() {
                ep.success_rate = rate;
            }
        }

        assert!(mm.is_learning());
    }
}
