use std::future::Future;
use std::pin::Pin;

use allowthem_core::{AuthError, EmailMessage, EmailSender};
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::SmtpConfig;

pub struct SmtpEmailSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpEmailSender {
    pub fn new(config: &SmtpConfig) -> Result<Self, eyre::Error> {
        let creds = Credentials::new(config.username.clone(), config.password.clone());

        let transport = if config.starttls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)?
        }
        .port(config.port)
        .credentials(creds)
        .build();

        let from: Mailbox = config
            .from
            .parse()
            .map_err(|e| eyre::eyre!("invalid SMTP from address: {e}"))?;

        Ok(Self { transport, from })
    }
}

impl EmailSender for SmtpEmailSender {
    fn send<'a>(
        &'a self,
        message: EmailMessage<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let to: Mailbox = message
                .to
                .parse()
                .map_err(|e| AuthError::Email(format!("invalid recipient: {e}")))?;

            let email = Message::builder()
                .from(self.from.clone())
                .to(to)
                .subject(message.subject)
                .header(ContentType::TEXT_PLAIN)
                .body(message.body.to_owned())
                .map_err(|e| AuthError::Email(format!("email build error: {e}")))?;

            self.transport
                .send(email)
                .await
                .map_err(|e| AuthError::Email(format!("SMTP send error: {e}")))?;

            Ok(())
        })
    }
}
