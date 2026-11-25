//! DVS: timecode vinyl drives the deck.
//!
//! [`decoder`] reads Serato Control Vinyl / Control CD (speed, direction,
//! absolute position); [`vinyl`] decides when the record has the deck and
//! where it puts the playhead (ABS / REL / INT, needle drops). Both are
//! ports of freedj-esp's (`src/timecode.cpp`, `p4/deck/main/vinyl.cpp`),
//! tuned there on a real turntable.
// SPDX-FileCopyrightText: 2026 Dan Newcome
// SPDX-License-Identifier: GPL-2.0-or-later

pub mod decoder;
pub mod vinyl;

pub use decoder::{Decoder, Format, SWITCH_PHASE, SWITCH_POLARITY, SWITCH_PRIMARY};
pub use vinyl::{Mode, Vinyl, VinylState};
