// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use ::flexi_logger::{with_thread, Logger};
use ::std::sync::Once;

//======================================================================================================================
// Static Variables
//======================================================================================================================

/// Guardian to the logging initialize function.
static INIT_LOG: Once = Once::new();

//======================================================================================================================
// Standalone Functions
//======================================================================================================================

/// Initializes logging features.
pub fn initialize() {
    INIT_LOG.call_once(|| {
        Logger::try_with_env().unwrap().format(with_thread).start().unwrap();
    });
}
