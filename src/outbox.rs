//! File d'attente persistante des mouvements.
//!
//! Chaque mouvement est écrit sur disque avant d'être envoyé à n8n. Si le
//! réseau, n8n ou l'application tombe, l'événement est rejoué au démarrage
//! suivant. L'`event_id` reste stable entre les tentatives, ce qui permet au
//! serveur de dédupliquer.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SENT_HISTORY: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    Pending,
    Sending,
    Sent,
    Rejected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedEvent {
    pub event_id: String,
    #[serde(default)]
    pub seq: u64,
    pub sku: String,
    pub action: String,
    pub qty: u32,
    pub station_id: String,
    pub scanned_at: String,
    pub raw: String,
    #[serde(default)]
    pub motif: Option<String>,
    #[serde(default)]
    pub cancels_event_id: Option<String>,
    pub status: EventStatus,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub next_attempt_ms: u64,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub stock_after: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub rejected_code: Option<String>,
    #[serde(default)]
    pub rejected_available: Option<i64>,
}

#[derive(Default, Serialize, Deserialize)]
struct OutboxFile {
    #[serde(default)]
    next_seq: u64,
    #[serde(default)]
    events: Vec<QueuedEvent>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutboxCounts {
    pub pending: usize,
    pub rejected: usize,
    pub sent: usize,
}

pub struct Outbox {
    path: PathBuf,
    next_seq: u64,
    events: Vec<QueuedEvent>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Outbox {
    pub fn load(path: PathBuf) -> Self {
        let mut outbox = Self {
            path,
            next_seq: 0,
            events: Vec::new(),
        };

        let Ok(data) = fs::read_to_string(&outbox.path) else {
            return outbox;
        };

        match serde_json::from_str::<OutboxFile>(&data) {
            Ok(file) => {
                outbox.next_seq = file.next_seq;
                outbox.events = file.events;
                // Un événement « sending » au démarrage signifie que
                // l'application s'est arrêtée pendant l'envoi : on le rejoue.
                for event in &mut outbox.events {
                    if event.status == EventStatus::Sending {
                        event.status = EventStatus::Pending;
                        event.next_attempt_ms = 0;
                    }
                }
                // Au cas où le fichier viendrait d'une version antérieure ou
                // aurait été édité à la main.
                outbox.next_seq = outbox.next_seq.max(
                    outbox
                        .events
                        .iter()
                        .map(|event| event.seq)
                        .max()
                        .unwrap_or(0),
                );
                outbox.prune_sent();
            }
            Err(error) => {
                eprintln!("File d'attente illisible ({error}), sauvegarde du fichier corrompu.");
                let backup = outbox.path.with_extension("json.corrupt");
                let _ = fs::rename(&outbox.path, backup);
            }
        }

        outbox
    }

    pub fn enqueue(&mut self, mut event: QueuedEvent) -> u64 {
        self.next_seq = self.next_seq.saturating_add(1).max(1);
        event.seq = self.next_seq;
        event.status = EventStatus::Pending;
        event.next_attempt_ms = 0;
        self.events.push(event);
        self.save();
        self.next_seq
    }

    /// Prend le prochain événement à envoyer et le marque « sending ».
    ///
    /// FIFO strict : si le plus ancien événement en attente est en backoff, on
    /// attend, même si un événement plus récent est prêt. Cela préserve l'ordre
    /// des mouvements de stock et évite qu'une sortie récente double une entrée
    /// plus ancienne en cours de réessai.
    pub fn next_ready(&mut self, now_ms: u64) -> Option<QueuedEvent> {
        let index = self
            .events
            .iter()
            .position(|event| event.status == EventStatus::Pending)?;

        if self.events[index].next_attempt_ms > now_ms {
            return None;
        }

        self.events[index].status = EventStatus::Sending;
        let event = self.events[index].clone();
        self.save();
        Some(event)
    }

    pub fn mark_sent(&mut self, event_id: &str, stock_after: Option<i64>, message: Option<String>) {
        if let Some(event) = self.events.iter_mut().find(|e| e.event_id == event_id) {
            event.status = EventStatus::Sent;
            event.stock_after = stock_after;
            event.message = message;
            event.last_error = None;
            event.rejected_code = None;
            event.rejected_available = None;
            self.prune_sent();
            self.save();
        }
    }

    pub fn mark_rejected(
        &mut self,
        event_id: &str,
        code: Option<String>,
        message: Option<String>,
        available: Option<i64>,
    ) {
        if let Some(event) = self.events.iter_mut().find(|e| e.event_id == event_id) {
            event.status = EventStatus::Rejected;
            event.rejected_code = code;
            event.rejected_available = available;
            event.last_error = message.clone();
            event.message = message;
            self.save();
        }
    }

    pub fn mark_retry(&mut self, event_id: &str, error: String, delay_ms: u64) {
        if let Some(event) = self.events.iter_mut().find(|e| e.event_id == event_id) {
            event.status = EventStatus::Pending;
            event.attempts = event.attempts.saturating_add(1);
            event.next_attempt_ms = now_ms().saturating_add(delay_ms);
            event.last_error = Some(error);
            self.save();
        }
    }

    /// Supprime un événement encore en attente (annulation locale avant envoi).
    /// Retourne faux si l'événement est déjà en cours d'envoi ou parti.
    pub fn remove_pending(&mut self, event_id: &str) -> bool {
        let Some(index) = self
            .events
            .iter()
            .position(|e| e.event_id == event_id && e.status == EventStatus::Pending)
        else {
            return false;
        };
        self.events.remove(index);
        self.save();
        true
    }

    /// Supprime l'historique des événements refusés (après acquittement).
    pub fn clear_rejected(&mut self) -> usize {
        let before = self.events.len();
        self.events
            .retain(|event| event.status != EventStatus::Rejected);
        let removed = before - self.events.len();
        if removed > 0 {
            self.save();
        }
        removed
    }

    pub fn counts(&self) -> OutboxCounts {
        let mut counts = OutboxCounts::default();
        for event in &self.events {
            match event.status {
                EventStatus::Pending | EventStatus::Sending => counts.pending += 1,
                EventStatus::Rejected => counts.rejected += 1,
                EventStatus::Sent => counts.sent += 1,
            }
        }
        counts
    }

    fn prune_sent(&mut self) {
        let sent_count = self
            .events
            .iter()
            .filter(|event| event.status == EventStatus::Sent)
            .count();
        if sent_count <= MAX_SENT_HISTORY {
            return;
        }

        let mut to_remove = sent_count - MAX_SENT_HISTORY;
        self.events.retain(|event| {
            if to_remove > 0 && event.status == EventStatus::Sent {
                to_remove -= 1;
                false
            } else {
                true
            }
        });
    }

    fn save(&self) {
        let data = match serde_json::to_string_pretty(&OutboxFile {
            next_seq: self.next_seq,
            events: self.events.clone(),
        }) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("Impossible de sérialiser la file d'attente : {error}");
                return;
            }
        };

        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let tmp = self.path.with_extension("json.tmp");
        if fs::write(&tmp, data).is_err() {
            return;
        }

        #[cfg(target_os = "windows")]
        {
            // `rename` échoue sur Windows si la destination existe.
            let _ = fs::remove_file(&self.path);
        }

        let _ = fs::rename(&tmp, &self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> QueuedEvent {
        QueuedEvent {
            event_id: id.to_string(),
            seq: 0,
            sku: "DCO001".to_string(),
            action: "OUT".to_string(),
            qty: 2,
            station_id: "POSTE-01".to_string(),
            scanned_at: "2026-10-26T09:00:00Z".to_string(),
            raw: "OUT|DCO001".to_string(),
            motif: None,
            cancels_event_id: None,
            status: EventStatus::Pending,
            attempts: 0,
            next_attempt_ms: 0,
            last_error: None,
            stock_after: None,
            message: None,
            rejected_code: None,
            rejected_available: None,
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("scanner_n8n_test_{}_{}.json", name, now_ms()))
    }

    #[test]
    fn persists_and_reloads_pending_events() {
        let path = temp_path("reload");
        let _ = std::fs::remove_file(&path);

        {
            let mut outbox = Outbox::load(path.clone());
            outbox.enqueue(sample("e1"));
            assert_eq!(outbox.counts().pending, 1);
        }

        let mut outbox = Outbox::load(path.clone());
        assert_eq!(outbox.counts().pending, 1);
        let event = outbox.next_ready(now_ms()).unwrap();
        assert_eq!(event.seq, 1);
        assert_eq!(event.event_id, "e1");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn retry_keeps_event_pending() {
        let path = temp_path("retry");
        let _ = std::fs::remove_file(&path);

        let mut outbox = Outbox::load(path.clone());
        outbox.enqueue(sample("e2"));
        let event = outbox.next_ready(now_ms()).unwrap();
        outbox.mark_retry(&event.event_id, "timeout".into(), 5_000);

        assert_eq!(outbox.counts().pending, 1);
        assert!(outbox.next_ready(now_ms()).is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn next_ready_waits_for_earlier_retry_to_preserve_order() {
        let path = temp_path("fifo");
        let _ = std::fs::remove_file(&path);

        let mut outbox = Outbox::load(path.clone());
        outbox.enqueue(sample("e1"));
        outbox.enqueue(sample("e2"));
        outbox.mark_retry("e1", "timeout".into(), 10_000);

        // e2 est prêt, mais e1 (plus ancien) est en backoff : on attend e1.
        assert!(outbox.next_ready(now_ms()).is_none());

        let event = outbox.next_ready(now_ms() + 11_000).unwrap();
        assert_eq!(event.event_id, "e1");
        outbox.mark_sent("e1", Some(10), None);

        let event = outbox.next_ready(now_ms()).unwrap();
        assert_eq!(event.event_id, "e2");

        let _ = std::fs::remove_file(&path);
    }
}
