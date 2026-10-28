//! The model downloads as the assistant holds them (step 2d-2): the review, the download task,
//! the rows and the models folder, started with the assistant and stopped with it.
use super::download::{
    client::{Client, Endpoints},
    disk::{Disk, NoFaults},
    review::Reviewer,
    rows::Rows,
    task::{Config, Downloader},
};
use super::download_rows::SqlRows;
use std::{path::Path, sync::Arc};

pub(crate) struct Downloads {
    pub reviewer: Reviewer,
    pub downloader: Downloader,
    pub rows: Arc<dyn Rows>,
    pub disk: Arc<Disk>,
}

impl Downloads {
    /// Opens (creating, mode 0700) the models folder and starts the downloads over the
    /// shipped endpoints.
    ///
    /// # Errors
    ///
    /// A models folder that is a link, not Mimik's or not private, a missing trust store, or
    /// a disk that cannot be reconciled, by message.
    pub(crate) async fn start(models_dir: &Path, store: crate::Store) -> Result<Self, String> {
        let models = models_dir.to_owned();
        let disk = tokio::task::spawn_blocking(move || {
            use std::os::unix::fs::DirBuilderExt;
            match std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&models)
            {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err("the models folder cannot be created".to_owned()),
            }
            Disk::open(&models, Arc::new(NoFaults)).map_err(|error| error.to_string())
        })
        .await
        .map_err(|_| "the models folder cannot be opened".to_owned())??;
        let client = Client::new(&Endpoints::shipped()).map_err(|error| error.to_string())?;
        Self::with(
            client,
            Arc::new(disk),
            Arc::new(SqlRows(store)),
            Config::default(),
        )
        .await
    }

    /// The downloads over `client`, `disk` and `rows`.
    ///
    /// # Errors
    ///
    /// A disk that cannot be reconciled with the rows, by message.
    pub(crate) async fn with(
        client: Client,
        disk: Arc<Disk>,
        rows: Arc<dyn Rows>,
        config: Config,
    ) -> Result<Self, String> {
        let downloader =
            Downloader::spawn(client.clone(), Arc::clone(&disk), Arc::clone(&rows), config).await?;
        Ok(Self {
            reviewer: Reviewer::new(client),
            downloader,
            rows,
            disk,
        })
    }
}
