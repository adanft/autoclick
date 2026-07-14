#![allow(dead_code)]

#[path = "support/mod.rs"]
mod support;

mod capture {
    include!("../src/capture.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;

        include!("unit/capture_tests.rs");
    }
}

mod config {
    include!("../src/config.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        include!("unit/config_tests.rs");
    }
}

mod input {
    include!("../src/input.rs");

    #[cfg(test)]
    mod tests {
        use super::*;

        include!("unit/input_tests.rs");
    }
}

mod matcher {
    mod collect {
        include!("../src/matcher/collect.rs");
    }

    mod engine {
        include!("../src/matcher/engine.rs");
    }

    mod prepare {
        include!("../src/matcher/prepare.rs");
    }

    use opencv::core::Mat;
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    pub use engine::scan_all;
    pub use prepare::prepare_rules;

    #[derive(Debug, Clone)]
    pub struct PreparedRule {
        pub target_template: String,
        pub template_path: PathBuf,
        pub template_size: (u32, u32),
        pub template_mat: Arc<Mat>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub struct MatchRegion {
        pub left: i32,
        pub top: i32,
        pub width: i32,
        pub height: i32,
    }

    pub type MatchSet = BTreeMap<String, Vec<MatchRegion>>;

    #[cfg(test)]
    mod tests {
        use super::collect::collect_regions;
        use super::engine::load_grayscale_mat;
        use super::prepare::prepare_rules_with_loader;
        use super::*;
        use image::{Rgba, RgbaImage};
        use opencv::core::{Mat, Scalar, CV_32FC1};
        use opencv::prelude::MatTrait;
        use std::sync::{Arc, Mutex};
        use tempfile::tempdir;

        include!("unit/matcher_tests.rs");
    }
}

mod monitor {
    include!("../src/monitor.rs");
}

mod wayland_pointer {
    include!("../src/wayland_pointer.rs");

    #[cfg(test)]
    mod tests {
        use super::*;

        include!("unit/wayland_pointer_tests.rs");
    }
}

mod rules {
    include!("../src/rules.rs");

    #[cfg(test)]
    mod tests {
        use super::*;

        include!("unit/rules_tests.rs");
    }
}

mod runtime {
    include!("../src/runtime.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::matcher::{MatchRegion, PreparedRule};
        use anyhow::anyhow;
        use opencv::core::{Mat, Scalar, CV_8UC1};
        use std::sync::{Arc, Mutex};

        include!("unit/runtime_tests.rs");
    }
}

mod app {
    mod bootstrap {
        include!("../src/app/bootstrap.rs");
    }

    mod prompts {
        include!("../src/app/prompts.rs");
    }

    mod summary {
        include!("../src/app/summary.rs");
    }

    pub(crate) use bootstrap::{
        create_wayland_backend_with, prepare_runtime_rules_with, run_with_io_and_monitors,
    };
    pub(crate) use prompts::{load_or_configure_with_io, prompt_for_config, PromptIo};
    pub(crate) use summary::render_startup_summary;

    #[cfg(test)]
    mod tests {
        use super::*;
        use anyhow::anyhow;
        use opencv::core::{Mat, Scalar, CV_8UC1};
        use std::collections::VecDeque;
        use std::fs;
        use tempfile::tempdir;

        include!("unit/app_tests.rs");
    }
}
