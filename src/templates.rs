use minijinja::Environment;
use std::path::PathBuf;

#[cfg(debug_assertions)]
use minijinja_autoreload::AutoReloader;

pub struct Templates {
    #[cfg(debug_assertions)]
    reloader: AutoReloader,
    #[cfg(not(debug_assertions))]
    env: Environment<'static>,
}

impl Templates {
    pub fn new(template_dir: PathBuf) -> Self {
        #[cfg(debug_assertions)]
        {
            let reloader = AutoReloader::new(move |notifier| {
                let mut env = Environment::new();
                minijinja_contrib::add_to_environment(&mut env);
                notifier.watch_path(&template_dir, true);
                env.set_loader(minijinja::path_loader(&template_dir));
                Ok(env)
            });
            Self { reloader }
        }

        #[cfg(not(debug_assertions))]
        {
            let _ = template_dir;
            let mut env = Environment::new();
            minijinja_contrib::add_to_environment(&mut env);
            minijinja_embed::load_templates!(&mut env);
            Self { env }
        }
    }

    pub fn render(&self, name: &str, ctx: minijinja::Value) -> Result<String, minijinja::Error> {
        #[cfg(debug_assertions)]
        {
            let env = self.reloader.acquire_env().map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("failed to acquire template environment: {e}"),
                )
            })?;
            env.get_template(name)?.render(ctx)
        }

        #[cfg(not(debug_assertions))]
        {
            self.env.get_template(name)?.render(ctx)
        }
    }

    pub fn default_dir() -> PathBuf {
        let manifest = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(manifest)
            .join("templates")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from("templates"))
    }
}

pub use minijinja::context;
