use crate::{
    SlackError,
    api::{ApiError, SlackApi},
    store::{DeliveryState, Store},
};
use renoa_local::LocalHost;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub(crate) struct Routines {
    pub(crate) host: LocalHost,
    pub(crate) store: Store,
    pub(crate) api: Arc<SlackApi>,
    pub(crate) shutdown: CancellationToken,
}
impl Routines {
    pub(crate) async fn run(self) -> Result<(), SlackError> {
        while !self.shutdown.is_cancelled() {
            self.project().await?;
            let delay = self.deliver_one().await?;
            crate::service::pause(&self.shutdown, delay).await;
        }
        Ok(())
    }
    pub(crate) async fn project(&self) -> Result<(), SlackError> {
        let after = self.store.routine_cursor().await?;
        for run in self.host.completed_routine_runs(after).await? {
            self.store.admit_routine_result(run).await?;
        }
        Ok(())
    }
    pub(crate) async fn deliver_one(&self) -> Result<Duration, SlackError> {
        let Some(delivery) = self.store.next_routine_delivery().await? else {
            return Ok(Duration::from_secs(1));
        };
        self.store
            .claim_routine_delivery(delivery.run_id.clone(), delivery.chunk)
            .await?;
        let (state, ts, error, delay) = match self.api.post(&delivery.topic, &delivery.text).await {
            Ok(sent) => (
                DeliveryState::Sent,
                Some(sent.ts),
                None,
                Duration::from_secs(1),
            ),
            Err(ApiError::RateLimited(delay)) => (DeliveryState::Pending, None, None, delay),
            Err(error) => {
                let state = if matches!(error, ApiError::Rejected(_)) {
                    DeliveryState::Failed
                } else {
                    DeliveryState::Unknown
                };
                eprintln!("Slack routine result delivery {}: {error}", state.as_str());
                (state, None, Some(error.to_string()), Duration::from_secs(1))
            }
        };
        self.store
            .finish_routine_delivery(delivery.run_id, delivery.chunk, state, ts, error)
            .await?;
        Ok(delay)
    }
}
