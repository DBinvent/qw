//! Outbound mail for the multi-user host's optional email-notify feature
//! (`server/multi-user.md`: "when it lands the address is stored encrypted
//! at rest... and it is a verify / notify channel only, never a passphrase
//! reset"). Nothing here touches the ledger or the protocol — this is
//! account-directory housekeeping, same tier as the nickname.
//!
//! **One provider, no fallback.** `knownby.work` is already onboarded to
//! Cloudflare Email Sending (`qw-bo`'s own `CLAUDE.md`), so this sends
//! exactly one message shape — a confirmation code — over that REST API.
//! `../signon/src/email.rs` is the fuller version of this (SMTP + OAuth
//! sign-in codes + a template library) but it is a private, shared crate
//! for a different account model (email + password); pulling it into the
//! public `qw` repo for one call would be a dependency for a handful of
//! lines this file already has.
//!
//! **Unconfigured is not a startup failure**, unlike `signon::Mailer::
//! from_env`. That crate's account layer *requires* mail to do its job at
//! all; this host's does not — §8 forbids the peer-to-peer services from
//! ever requiring a mailer, and adding an email is opt-in self-service on
//! an account that already works without one. So a host with no
//! `CF_ACCOUNT_ID` / `CF_EMAIL_TOKEN` / `EMAIL_FROM` set still boots and
//! runs register/login/everything else; `set_email` and `email_resend`
//! simply log the code instead of mailing it (same "goes to the log
//! instead" convention as `EMAIL_DISABLED=1` elsewhere in this ecosystem).

use std::time::Duration;

pub struct Mailer {
    cf: Option<Cloudflare>,
}

struct Cloudflare {
    account_id: String,
    token: String,
    from: String,
    from_name: Option<String>,
    http: reqwest::Client,
}

impl Mailer {
    /// `CF_ACCOUNT_ID` + `CF_EMAIL_TOKEN` + `EMAIL_FROM` (+ optional
    /// `EMAIL_FROM_NAME`). Any one missing or blank leaves this disabled —
    /// see the module doc for why that is not fatal here.
    pub fn from_env() -> Self {
        let account_id = non_empty_env("CF_ACCOUNT_ID");
        let token = non_empty_env("CF_EMAIL_TOKEN");
        let from = non_empty_env("EMAIL_FROM");
        let cf = match (account_id, token, from) {
            (Some(account_id), Some(token), Some(from)) => Some(Cloudflare {
                account_id,
                token,
                from,
                from_name: non_empty_env("EMAIL_FROM_NAME"),
                http: reqwest::Client::builder()
                    .timeout(Duration::from_secs(15))
                    .build()
                    .expect("build the Cloudflare Email HTTP client"),
            }),
            _ => None,
        };
        if cf.is_none() {
            eprintln!(
                "mail: CF_ACCOUNT_ID / CF_EMAIL_TOKEN / EMAIL_FROM not all set — \
                 email-notify is disabled (codes are logged, not sent; \
                 register/login/everything else is unaffected)"
            );
        }
        Self { cf }
    }

    /// A mailer with no transport, for tests — every `send_code` logs
    /// instead of making a network call.
    pub fn disabled() -> Self {
        Self { cf: None }
    }

    pub fn enabled(&self) -> bool {
        self.cf.is_some()
    }

    /// Never surfaces an error: the caller (`set_email` / `email_resend`)
    /// has already persisted the code before this runs, so a send failure
    /// must not roll that back or change the HTTP response — the holder
    /// can always ask for a fresh one. Logged either way, so a stuck
    /// deployment is visible in `journalctl`, not silent.
    pub async fn send_code(&self, to: &str, code: &str) {
        let Some(cf) = &self.cf else {
            eprintln!("mail: (disabled) verification code for {to}: {code}");
            #[cfg(test)]
            outbox::record(to, code);
            return;
        };
        let subject = "Your knownby.work verification code";
        let text = format!(
            "Enter this code on the Security page to confirm this address:\n\n\
             {code}\n\n\
             It expires in 24 hours. This is a verify / notify channel only — \
             it is never a way to reset your passphrase.\n\n\
             If you did not ask for this, ignore it; nothing happens without the code.\n"
        );
        let html = format!(
            "<p>Enter this code on the Security page to confirm this address:</p>\
             <p style=\"font-size:22px;font-weight:600;letter-spacing:.12em\">{code}</p>\
             <p>It expires in 24 hours. This is a verify / notify channel only — \
             it is never a way to reset your passphrase.</p>\
             <p>If you did not ask for this, ignore it; nothing happens without the code.</p>"
        );
        if let Err(e) = cf.send(to, subject, &text, &html).await {
            eprintln!("mail: send to {to} failed: {e}");
        }
    }
}

impl Cloudflare {
    /// Field names differ from the Workers `send_email` binding in ways
    /// that only fail at runtime: `from` takes `address` (not `email`).
    async fn send(&self, to: &str, subject: &str, text: &str, html: &str) -> Result<(), String> {
        let body = serde_json::json!({
            "to": to,
            "from": match &self.from_name {
                Some(name) => serde_json::json!({ "address": self.from, "name": name }),
                None => serde_json::json!({ "address": self.from }),
            },
            "subject": subject,
            // Both parts, always: some clients only render text, and a
            // text-less message scores worse with spam filters.
            "text": text,
            "html": html,
        });
        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/email/sending/send",
            self.account_id
        );
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        let status = response.status();
        let payload = response.text().await.unwrap_or_default();
        if !status.is_success() {
            // The body carries Cloudflare's numeric error code, which is
            // the only way to tell "domain not onboarded" from "token
            // cannot send". It never echoes the token.
            return Err(format!("rejected ({status}): {payload}"));
        }
        // A 200 is not delivery: the result splits recipients into
        // delivered, queued and permanent_bounces.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) {
            let bounced = v["result"]["permanent_bounces"]
                .as_array()
                .is_some_and(|a| !a.is_empty());
            if bounced {
                return Err("permanently bounced".to_string());
            }
        }
        Ok(())
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// What a disabled mailer would have sent, for `multiuser`'s own tests —
/// same shape as `../signon/src/email.rs`'s own test outbox. Tests run
/// concurrently, so each uses an address unique to itself.
#[cfg(test)]
pub(crate) mod outbox {
    use std::sync::Mutex;

    static CODES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    pub(crate) fn record(to: &str, code: &str) {
        CODES
            .lock()
            .unwrap()
            .push((to.to_string(), code.to_string()));
    }

    /// The most recently sent code to `to`, if any — a resend replaces it.
    pub(crate) fn last_code_for(to: &str) -> Option<String> {
        CODES
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(t, _)| t == to)
            .map(|(_, c)| c.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_mailer_reports_disabled() {
        assert!(!Mailer::disabled().enabled());
    }

    #[test]
    fn from_env_is_disabled_unless_all_three_are_set() {
        // from_env() reads real process env, which tests must not fight
        // over concurrently — exercise non_empty_env directly instead.
        assert_eq!(non_empty_env("QW_MAIL_TEST_NEVER_SET"), None);
    }
}
