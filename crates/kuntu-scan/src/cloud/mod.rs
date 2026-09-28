mod engine;
mod guard;
mod model;
mod platform;
mod policy;

pub use engine::{
  DEFAULT_EVICTION_CONCURRENCY, EvictionCandidate, EvictionOutcome, EvictionPlan, EvictionProgress,
  EvictionProgressState, EvictionResult, EvictionStatus, ItemSummary, MAX_EVICTION_CONCURRENCY,
  PlanBudget, ScanOptions, SkippedItem, build_eviction_plan, build_eviction_plan_bounded,
  execute_eviction_plan, execute_eviction_plan_with_concurrency, execute_eviction_plan_with_state,
  execute_eviction_plan_with_state_and_observer, inspect_item, request_download_one,
};
pub use model::{
  Bytes, CloudError, DownloadState, ErrorKind, Fingerprint, ItemInfo, ItemKind, NativeError,
};
pub use platform::{CloudBackend, NativeICloudBackend};
pub use policy::{SkipReason, eviction_skip_reason};

#[cfg(test)]
mod tests;
