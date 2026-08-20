//! Startup harness discovery: probe every candidate `(harness, command)`
//! pair once — version and sign-in, in parallel, under a strict timeout —
//! and report what a team could be built from. Probes are quota-free by
//! construction (version/status commands only; never trial prompts).

use super::agent::{Agent, AuthState};
use super::descriptor::Capabilities;
use super::registry;
use super::runner::HarnessAgent;
use super::HarnessKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// One probed `(harness, command)` pair, as reported to the UI. `available`
/// means the binary answered its version probe; sign-in state and role
/// eligibility are separate facts so the picker can explain each precisely.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveredHarness {
    pub harness: HarnessKind,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub auth: AuthState,
    pub available: bool,
    /// Actionable explanation when something is off (missing binary,
    /// timeout, signed out) — picker copy, not log spam.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Selection disclosure (e.g. a workspace-trust flag the invocation
    /// passes); picking the harness is the opt-in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub lead_eligible: bool,
    pub teammate_eligible: bool,
    pub capabilities: Capabilities,
}

/// Cached probe results, keyed by `(harness, command)` so the selected
/// slots never pay for a second process spawn.
pub struct Discovery {
    pub harnesses: Vec<DiscoveredHarness>,
    cache: HashMap<(HarnessKind, String), Probe>,
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub version: Option<String>,
    pub auth: AuthState,
    pub reason: Option<String>,
}

impl Discovery {
    pub fn probe(&self, harness: HarnessKind, command: &str) -> Option<&Probe> {
        self.cache.get(&(harness, command.to_owned()))
    }
}

/// Probe one candidate. The timeout guards the version probe (the auth
/// probe carries its own internal timeout and only runs once the binary
/// proved responsive).
pub async fn probe_one(harness: HarnessKind, command: &str, timeout: Duration) -> Probe {
    let descriptor = registry::descriptor(harness);
    let agent = HarnessAgent::new(descriptor, command);
    let version = match tokio::time::timeout(timeout, agent.version()).await {
        Ok(Ok(v)) => Ok(v.raw),
        Ok(Err(e)) => Err(format!(
            "not installed: {} ({e:#})",
            descriptor.install_hint
        )),
        Err(_) => Err(format!(
            "version probe timed out after {}s — is `{command}` responsive?",
            timeout.as_secs()
        )),
    };
    match version {
        Err(reason) => Probe {
            version: None,
            auth: AuthState::ProbeFailed,
            reason: Some(reason),
        },
        Ok(version) => {
            let auth = agent.auth_status().await;
            let reason = (auth == AuthState::Unauthenticated)
                .then(|| format!("not signed in: {}", descriptor.login_hint));
            Probe {
                version: Some(version),
                auth,
                reason,
            }
        }
    }
}

/// Probe every candidate in parallel, deduplicated by `(harness, command)`.
/// Candidate order is preserved in the report.
pub async fn discover(candidates: Vec<(HarnessKind, String)>, timeout: Duration) -> Discovery {
    let mut unique: Vec<(HarnessKind, String)> = Vec::new();
    for candidate in candidates {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }

    let mut set = tokio::task::JoinSet::new();
    for (harness, command) in unique.clone() {
        set.spawn(async move {
            let probe = probe_one(harness, &command, timeout).await;
            ((harness, command), probe)
        });
    }
    let mut cache: HashMap<(HarnessKind, String), Probe> = HashMap::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((key, probe)) = joined {
            cache.insert(key, probe);
        }
    }

    let fallback = Probe {
        version: None,
        auth: AuthState::ProbeFailed,
        reason: Some("probe task failed".to_owned()),
    };
    let harnesses = unique
        .iter()
        .map(|key| {
            let (harness, command) = key;
            let probe = cache.get(key).unwrap_or(&fallback);
            let descriptor = registry::descriptor(*harness);
            let capabilities = descriptor.capabilities;
            DiscoveredHarness {
                harness: *harness,
                command: command.clone(),
                version: probe.version.clone(),
                auth: probe.auth,
                available: probe.version.is_some(),
                reason: probe.reason.clone(),
                note: descriptor.selection_note.map(str::to_owned),
                lead_eligible: capabilities.lead_eligible(),
                teammate_eligible: capabilities.teammate_eligible(),
                capabilities,
            }
        })
        .collect();

    Discovery { harnesses, cache }
}
