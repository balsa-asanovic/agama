//! This module offers a mechanism to report the installation progress in Agama's command-line
//! interface.
//!
//! The library does not prescribe any way to present that information to the user. As shown in the
//! example below, you can build your own presenter and implement the [ProgressPresenter] trait.
//!
//! ```no_run
//! # use agama_lib::progress::{Progress, ProgressMonitor, ProgressPresenter};
//! # use async_std::task::block_on;
//! # use zbus;
//!
//! // Custom presenter
//! struct SimplePresenter {};
//!
//! impl SimplePresenter {
//!   fn report_progress(&self, progress: &Progress) {
//!       println!("{}/{} {}", &progress.current_step, &progress.max_steps, &progress.title);
//!   }
//! }
//!
//! impl ProgressPresenter for SimplePresenter {
//!     fn start(&mut self, progress: &Progress) {
//!        println!("Starting...");
//!        self.report_progress(progress);
//!     }
//!
//!     fn update(&mut self, progress: &Progress) {
//!        self.report_progress(progress);
//!     }
//!
//!     fn finish(&mut self) {
//!         println!("Done");
//!     }
//! }
//!
//! let connection = block_on(zbus::Connection::system()).unwrap();
//! let mut monitor = block_on(ProgressMonitor::new(connection)).unwrap();
//! monitor.run(SimplePresenter {});
//! ```

use crate::error::ServiceError;
use crate::proxies::ProgressProxy;
use futures::stream::{SelectAll, StreamExt};
use futures_util::{future::try_join3, Stream};
use std::error::Error;
use zbus::Connection;

/// Represents the progress for an Agama service.
#[derive(Default, Debug)]
pub struct Progress {
    /// Current step
    pub current_step: u32,
    /// Number of steps
    pub max_steps: u32,
    /// Title of the current step
    pub current_title: String,
    /// Whether the progress reporting is finished
    pub finished: bool,
}

impl Progress {
    pub async fn from_proxy(proxy: &crate::proxies::ProgressProxy<'_>) -> zbus::Result<Progress> {
        let ((current_step, current_title), max_steps, finished) =
            try_join3(proxy.current_step(), proxy.total_steps(), proxy.finished()).await?;

        Ok(Self {
            current_step,
            current_title,
            max_steps,
            finished,
        })
    }
}

/// Monitorizes and reports the progress of Agama's current operation.
///
/// It implements a main/details reporter by listening to the manager and software services,
/// similar to Agama's web UI. How this information is displayed depends on the presenter (see
/// [ProgressMonitor.run]).
pub struct ProgressMonitor<'a> {
    manager_proxy: ProgressProxy<'a>,
    software_proxy: ProgressProxy<'a>,
}

impl<'a> ProgressMonitor<'a> {
    pub async fn new(connection: Connection) -> Result<ProgressMonitor<'a>, ServiceError> {
        let manager_proxy = ProgressProxy::builder(&connection)
            .path("/org/opensuse/Agama1/Manager")?
            .destination("org.opensuse.Agama1")?
            .build()
            .await?;

        let software_proxy = ProgressProxy::builder(&connection)
            .path("/org/opensuse/Agama/Software1")?
            .destination("org.opensuse.Agama.Software1")?
            .build()
            .await?;

        Ok(Self {
            manager_proxy,
            software_proxy,
        })
    }

    /// Runs the monitor until the current operation finishes.
    pub async fn run(
        &mut self,
        mut presenter: impl ProgressPresenter,
    ) -> Result<(), Box<dyn Error>> {
        presenter.start(&self.main_progress().await);
        let mut changes = self.build_stream().await;

        while let Some(stream) = changes.next().await {
            match stream {
                "/org/opensuse/Agama1/Manager" => {
                    let progress = self.main_progress().await;
                    if progress.finished {
                        presenter.finish();
                        return Ok(());
                    } else {
                        presenter.update_main(&progress);
                    }
                }
                "/org/opensuse/Agama/Software1" => {
                    presenter.update_detail(&self.detail_progress().await)
                }
                _ => eprintln!("Unknown"),
            };
        }

        Ok(())
    }

    /// Proxy that reports the progress.
    async fn main_progress(&self) -> Progress {
        Progress::from_proxy(&self.manager_proxy).await.unwrap()
    }

    /// Proxy that reports the progress detail.
    async fn detail_progress(&self) -> Progress {
        Progress::from_proxy(&self.software_proxy).await.unwrap()
    }

    /// Builds an stream of progress changes.
    ///
    /// It listens for changes in the `Current` property and generates a stream identifying the
    /// proxy where the change comes from.
    async fn build_stream(&self) -> SelectAll<impl Stream<Item = &str> + '_> {
        let mut streams = SelectAll::new();

        let proxies = [&self.manager_proxy, &self.software_proxy];
        for proxy in proxies.iter() {
            let stream = proxy.receive_current_step_changed().await;
            let path = proxy.path().as_str();
            let tagged = stream.map(move |_| path);
            streams.push(tagged);
        }
        streams
    }
}

/// Presents the progress to the user.
pub trait ProgressPresenter {
    /// Starts the progress reporting.
    ///
    /// * `progress`: current main progress.
    fn start(&mut self, progress: &Progress);

    /// Updates the progress.
    ///
    /// * `progress`: current progress.
    fn update_main(&mut self, progress: &Progress);

    /// Updates the progress detail.
    ///
    /// * `progress`: current progress detail.
    fn update_detail(&mut self, progress: &Progress);

    /// Finishes the progress reporting.
    fn finish(&mut self);
}
