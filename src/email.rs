use std::future::Future;
use std::pin::Pin;

use allowthem_core::{
    AuthError, EmailBranding, EmailMessage, EmailSender, SmtpConfig as AllowThemSmtpConfig,
    SmtpEmailSender as AllowThemSmtpEmailSender, SmtpTls,
};
use lettre::message::Mailbox;

use crate::config::SmtpConfig;

pub struct SmtpEmailSender {
    inner: AllowThemSmtpEmailSender,
}

impl SmtpEmailSender {
    pub fn new(config: &SmtpConfig) -> Result<Self, eyre::Error> {
        let inner = AllowThemSmtpEmailSender::new(build_smtp_config(config)?, email_branding())
            .map_err(|e| eyre::eyre!("invalid SMTP config: {e}"))?;

        Ok(Self { inner })
    }
}

impl EmailSender for SmtpEmailSender {
    fn send<'a>(
        &'a self,
        message: &'a EmailMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'a>> {
        self.inner.send(message)
    }
}

fn build_smtp_config(config: &SmtpConfig) -> Result<AllowThemSmtpConfig, eyre::Error> {
    let from: Mailbox = config
        .from
        .parse()
        .map_err(|e| eyre::eyre!("invalid SMTP from address: {e}"))?;

    Ok(AllowThemSmtpConfig {
        host: config.host.clone(),
        port: config.port,
        username: Some(config.username.clone()),
        password: Some(config.password.clone()),
        from_address: from.email.to_string(),
        from_name: from.name,
        tls: if config.starttls {
            SmtpTls::StartTls
        } else {
            SmtpTls::ImplicitTls
        },
    })
}

fn email_branding() -> EmailBranding {
    EmailBranding {
        app_name: "sendword".to_owned(),
        logo_url: None,
        footer_line: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smtp_config(from: &str, starttls: bool) -> SmtpConfig {
        SmtpConfig {
            host: "smtp.example.com".to_owned(),
            port: 587,
            username: "user".to_owned(),
            password: "pass".to_owned(),
            from: from.to_owned(),
            starttls,
        }
    }

    #[test]
    fn build_smtp_config_preserves_display_name_sender() {
        let config = build_smtp_config(&smtp_config("Sendword <sendword@example.com>", true))
            .expect("valid SMTP config");

        assert_eq!(config.host, "smtp.example.com");
        assert_eq!(config.port, 587);
        assert_eq!(config.username.as_deref(), Some("user"));
        assert_eq!(config.password.as_deref(), Some("pass"));
        assert_eq!(config.from_address, "sendword@example.com");
        assert_eq!(config.from_name.as_deref(), Some("Sendword"));
        assert_eq!(config.tls, SmtpTls::StartTls);
    }

    #[test]
    fn build_smtp_config_uses_implicit_tls_when_starttls_is_disabled() {
        let config = build_smtp_config(&smtp_config("sendword@example.com", false))
            .expect("valid SMTP config");

        assert_eq!(config.from_address, "sendword@example.com");
        assert_eq!(config.from_name, None);
        assert_eq!(config.tls, SmtpTls::ImplicitTls);
    }

    #[test]
    fn build_smtp_config_rejects_invalid_sender_address() {
        let err = build_smtp_config(&smtp_config("not an email address", true))
            .expect_err("invalid sender should fail");

        assert!(
            err.to_string().contains("invalid SMTP from address"),
            "unexpected error: {err}"
        );
    }
}
