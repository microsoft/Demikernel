// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::runtime::fail::Fail;
use ::libc::ERANGE;
use ::std::num::NonZeroU16;

//======================================================================================================================
// Structures
//======================================================================================================================

/// Port Number
#[derive(Eq, PartialEq, Hash, Copy, Clone, Debug, Ord, PartialOrd)]
pub struct Port16(NonZeroU16);

//======================================================================================================================
// Associate Functions
//======================================================================================================================

impl Port16 {
    pub fn new(num: NonZeroU16) -> Self {
        Self { 0: num }
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl From<Port16> for u16 {
    fn from(val: Port16) -> Self {
        u16::from(val.0)
    }
}

impl TryFrom<u16> for Port16 {
    type Error = Fail;

    fn try_from(n: u16) -> Result<Self, Fail> {
        Ok(Port16(
            NonZeroU16::new(n).ok_or(Fail::new(ERANGE, "port number may not be zero"))?,
        ))
    }
}
