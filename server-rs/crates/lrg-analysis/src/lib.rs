//! Clustering, person matching, group_and_sort, knee filter, edit-style
//! training aggregation. Style engine (LLM-free interpolation) and
//! keyword clustering land later in M8.

pub mod clustering;
pub mod culling_config;
pub mod face_aggregate;
pub mod grouping;
pub mod persons;
pub mod relevance_filter;
pub mod training;
