use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureEcosystem {
    Cargo,
    Npm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureNodeKind {
    Application,
    Library,
    Package,
    Service,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchitectureNode {
    pub id: String,
    pub kind: ArchitectureNodeKind,
    pub name: String,
    pub manifest_path: String,
    pub ecosystem: ArchitectureEcosystem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchitectureEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureRepositoryStatus {
    Error,
    Partial,
    Ready,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryArchitecture {
    pub repository_id: String,
    pub name: String,
    pub status: ArchitectureRepositoryStatus,
    pub nodes: Vec<ArchitectureNode>,
    pub edges: Vec<ArchitectureEdge>,
    pub errors: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectArchitecture {
    pub project_id: String,
    pub repositories: Vec<RepositoryArchitecture>,
    pub scanned_at_unix_ms: u64,
    pub stale: bool,
    pub truncated: bool,
}
