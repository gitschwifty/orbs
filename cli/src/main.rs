use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use orbs::dep::{DepEdge, EdgeType};
use orbs::dep_store::DepStore;
use orbs::id::OrbId;
use orbs::orb::{priority_name, Orb, OrbPhase, OrbStatus, OrbType};
use orbs::orb_store::OrbStore;
use orbs::tree::{build_orb_tree, OrbNode};

#[derive(Parser)]
#[command(name = "orbs", version, about = "Local durable work item store")]
struct Cli {
    /// Store directory containing orbs.jsonl and deps.jsonl.
    #[arg(long, global = true, env = "ORBS_STATE_DIR", default_value = ".orbs")]
    state_dir: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create the local store directory and empty JSONL files.
    Init,
    /// Create a new orb.
    Create {
        /// Title for the orb.
        title: String,
        /// Description. Defaults to the title.
        #[arg(short, long)]
        description: Option<String>,
        /// Orb type: task, epic, feature, bug, chore, docs, or a custom value.
        #[arg(short = 't', long = "type", default_value = "task")]
        orb_type: String,
        /// Priority, clamped to 1..=5.
        #[arg(short, long, default_value_t = 3)]
        priority: u8,
        /// Parent orb id.
        #[arg(long)]
        parent: Option<String>,
        /// Root orb id. Defaults to --parent when parent is set.
        #[arg(long)]
        root: Option<String>,
        /// Create as draft instead of pending.
        #[arg(long)]
        draft: bool,
        /// Attach a label. Repeatable.
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
    },
    /// List orbs.
    List {
        /// Filter by type.
        #[arg(short = 't', long = "type")]
        orb_type: Option<String>,
        /// Filter by effective status or exact phase/status.
        #[arg(short, long)]
        status: Option<String>,
        /// Include tombstoned orbs.
        #[arg(long)]
        all: bool,
        /// Show only orbs with at least one of these labels.
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
    },
    /// Show one orb.
    Show {
        /// Orb id.
        id: String,
    },
    /// Update editable fields on an orb.
    Update {
        /// Orb id.
        id: String,
        /// New title.
        #[arg(long)]
        title: Option<String>,
        /// New description.
        #[arg(long)]
        description: Option<String>,
        /// New priority, clamped to 1..=5.
        #[arg(short, long)]
        priority: Option<u8>,
        /// New lifecycle status or phase.
        #[arg(short, long)]
        status: Option<String>,
        /// Defer the orb if its lifecycle allows it.
        #[arg(long)]
        defer: bool,
        /// Restore a deferred orb to its default active queue state.
        #[arg(long)]
        undefer: bool,
        /// Add a label. Repeatable.
        #[arg(long = "add-label", value_name = "LABEL")]
        add_labels: Vec<String>,
        /// Remove a label. Repeatable.
        #[arg(long = "remove-label", value_name = "LABEL")]
        remove_labels: Vec<String>,
        /// Replace labels with a comma-separated list.
        #[arg(long = "set-labels", value_name = "CSV")]
        set_labels: Option<String>,
    },
    /// Soft-delete an orb.
    Delete {
        /// Orb id.
        id: String,
        /// Deletion reason.
        #[arg(short, long)]
        reason: Option<String>,
    },
    /// Manage dependency edges.
    Dep {
        #[command(subcommand)]
        action: DepAction,
    },
    /// List dependency edges touching an orb.
    Deps {
        /// Orb id.
        id: String,
    },
    /// Show orbs whose blockers are complete.
    Ready,
    /// Show orbs blocked by incomplete dependencies.
    Waiting,
    /// Show topological pipeline order.
    Pipeline,
    /// Print a parent/child tree from a root orb.
    Tree {
        /// Root orb id.
        id: String,
    },
}

#[derive(Subcommand)]
enum DepAction {
    /// Add an edge.
    Add {
        from: String,
        to: String,
        /// Edge type: blocks, depends_on, parent, child, related, duplicates, follows.
        #[arg(short = 't', long = "type", default_value = "blocks")]
        edge_type: String,
    },
    /// Remove an edge.
    Rm {
        from: String,
        to: String,
        /// Edge type: blocks, depends_on, parent, child, related, duplicates, follows.
        #[arg(short = 't', long = "type", default_value = "blocks")]
        edge_type: String,
    },
    /// List every active edge.
    List,
}

struct Stores {
    state_dir: PathBuf,
    orbs: OrbStore,
    deps: DepStore,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let stores = stores(&cli.state_dir);

    match cli.command {
        Commands::Init => cmd_init(&stores),
        Commands::Create {
            title,
            description,
            orb_type,
            priority,
            parent,
            root,
            draft,
            labels,
        } => cmd_create(
            &stores,
            &title,
            description.as_deref().unwrap_or(&title),
            parse_orb_type(&orb_type),
            priority,
            parent.as_deref(),
            root.as_deref(),
            draft,
            labels,
        ),
        Commands::List {
            orb_type,
            status,
            all,
            labels,
        } => cmd_list(&stores, orb_type.as_deref(), status.as_deref(), all, labels),
        Commands::Show { id } => cmd_show(&stores, &id),
        Commands::Update {
            id,
            title,
            description,
            priority,
            status,
            defer,
            undefer,
            add_labels,
            remove_labels,
            set_labels,
        } => cmd_update(
            &stores,
            &id,
            title,
            description,
            priority,
            status.as_deref(),
            defer,
            undefer,
            add_labels,
            remove_labels,
            set_labels,
        ),
        Commands::Delete { id, reason } => cmd_delete(&stores, &id, reason),
        Commands::Dep { action } => cmd_dep(&stores, action),
        Commands::Deps { id } => cmd_deps(&stores, &id),
        Commands::Ready => cmd_ids_from_query(&stores, Query::Ready),
        Commands::Waiting => cmd_ids_from_query(&stores, Query::Waiting),
        Commands::Pipeline => cmd_ids_from_query(&stores, Query::Pipeline),
        Commands::Tree { id } => cmd_tree(&stores, &id),
    }
}

fn stores(state_dir: &Path) -> Stores {
    Stores {
        state_dir: state_dir.to_path_buf(),
        orbs: OrbStore::new(state_dir.join("orbs.jsonl")),
        deps: DepStore::new(state_dir.join("deps.jsonl")),
    }
}

fn ensure_state_dir(stores: &Stores) -> anyhow::Result<()> {
    std::fs::create_dir_all(&stores.state_dir)
        .with_context(|| format!("failed to create {}", stores.state_dir.display()))
}

fn touch(path: &Path) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    Ok(())
}

fn cmd_init(stores: &Stores) -> anyhow::Result<()> {
    ensure_state_dir(stores)?;
    touch(stores.orbs.path())?;
    touch(stores.deps.path())?;
    touch(&stores.state_dir.join("events.jsonl"))?;
    println!("Initialized {}", stores.state_dir.display());
    println!("  {}", stores.orbs.path().display());
    println!("  {}", stores.deps.path().display());
    println!("  {}", stores.state_dir.join("events.jsonl").display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_create(
    stores: &Stores,
    title: &str,
    description: &str,
    orb_type: OrbType,
    priority: u8,
    parent: Option<&str>,
    root: Option<&str>,
    draft: bool,
    labels: Vec<String>,
) -> anyhow::Result<()> {
    ensure_state_dir(stores)?;
    let mut orb = Orb::new(title, description)
        .with_type(orb_type)
        .with_priority(priority);

    if let Some(parent) = parent {
        orb = orb.with_parent(OrbId::from_raw(parent), root.map(OrbId::from_raw));
    }
    if draft {
        if orb.orb_type.uses_phase() {
            orb.phase = Some(OrbPhase::Draft);
        } else {
            orb.status = Some(OrbStatus::Draft);
        }
    }
    orb.labels = normalize_labels(labels);
    orb.update_content_hash();

    stores.orbs.append(&orb).context("failed to write orb")?;
    println!("Created {}", orb.id);
    print_orb_summary(&orb);
    Ok(())
}

fn cmd_list(
    stores: &Stores,
    orb_type: Option<&str>,
    status: Option<&str>,
    all: bool,
    labels: Vec<String>,
) -> anyhow::Result<()> {
    let mut orbs = if all {
        stores.orbs.load_all_including_tombstoned()
    } else {
        stores.orbs.load_all()
    }
    .context("failed to read orbs")?;

    if let Some(orb_type) = orb_type {
        let expected = parse_orb_type(orb_type);
        orbs.retain(|orb| orb.orb_type == expected);
    }
    if let Some(status) = status {
        let filter = parse_lifecycle_filter(status)?;
        orbs.retain(|orb| lifecycle_matches(orb, filter));
    }
    let labels = normalize_labels(labels);
    if !labels.is_empty() {
        orbs.retain(|orb| labels.iter().any(|label| orb.labels.contains(label)));
    }

    if orbs.is_empty() {
        println!("No orbs");
        return Ok(());
    }

    for orb in orbs {
        println!(
            "{} [{}] {} p{} {}",
            orb.id,
            orb_type_label(&orb.orb_type),
            lifecycle_label(&orb),
            orb.priority,
            orb.title
        );
    }
    Ok(())
}

fn cmd_show(stores: &Stores, id: &str) -> anyhow::Result<()> {
    let orb = load_orb(stores, id)?;
    print_orb_detail(&orb);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_update(
    stores: &Stores,
    id: &str,
    title: Option<String>,
    description: Option<String>,
    priority: Option<u8>,
    status: Option<&str>,
    defer: bool,
    undefer: bool,
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    set_labels: Option<String>,
) -> anyhow::Result<()> {
    ensure_state_dir(stores)?;
    if defer && undefer {
        bail!("--defer and --undefer cannot be used together");
    }

    let mut orb = load_orb(stores, id)?;
    if let Some(title) = title {
        orb.title = title;
    }
    if let Some(description) = description {
        orb.description = description;
    }
    if let Some(priority) = priority {
        orb.priority = priority.clamp(1, 5);
    }
    if let Some(status) = status {
        apply_lifecycle(&mut orb, status)?;
    }
    if defer && !orb.defer() {
        bail!("orb cannot be deferred from {}", lifecycle_label(&orb));
    }
    if undefer {
        orb.undefer();
    }

    if let Some(labels) = set_labels {
        orb.labels = normalize_labels(split_csv(&labels));
    }
    for label in normalize_labels(add_labels) {
        if !orb.labels.contains(&label) {
            orb.labels.push(label);
        }
    }
    let remove_labels = normalize_labels(remove_labels);
    if !remove_labels.is_empty() {
        orb.labels.retain(|label| !remove_labels.contains(label));
    }

    orb.update_content_hash();
    stores.orbs.update(&orb).context("failed to update orb")?;
    println!("Updated {}", orb.id);
    print_orb_summary(&orb);
    Ok(())
}

fn cmd_delete(stores: &Stores, id: &str, reason: Option<String>) -> anyhow::Result<()> {
    ensure_state_dir(stores)?;
    let mut orb = load_orb(stores, id)?;
    orb.tombstone(reason);
    stores
        .orbs
        .update(&orb)
        .context("failed to tombstone orb")?;
    println!("Deleted {}", orb.id);
    Ok(())
}

fn cmd_dep(stores: &Stores, action: DepAction) -> anyhow::Result<()> {
    ensure_state_dir(stores)?;
    match action {
        DepAction::Add {
            from,
            to,
            edge_type,
        } => {
            let edge = DepEdge::new(
                OrbId::from_raw(&from),
                OrbId::from_raw(&to),
                parse_edge_type(&edge_type)?,
            );
            stores.deps.add_edge(edge).context("failed to add edge")?;
            println!(
                "Added {from} -{}-> {to}",
                edge_type_label(parse_edge_type(&edge_type)?)
            );
        }
        DepAction::Rm {
            from,
            to,
            edge_type,
        } => {
            let edge_type = parse_edge_type(&edge_type)?;
            let removed = stores
                .deps
                .remove_edge(&OrbId::from_raw(&from), &OrbId::from_raw(&to), edge_type)
                .context("failed to remove edge")?;
            if removed {
                println!("Removed {from} -{}-> {to}", edge_type_label(edge_type));
            } else {
                println!("No matching edge");
            }
        }
        DepAction::List => print_edges(stores.deps.all_edges().context("failed to read deps")?),
    }
    Ok(())
}

fn cmd_deps(stores: &Stores, id: &str) -> anyhow::Result<()> {
    let id = OrbId::from_raw(id);
    let mut edges = stores.deps.edges_from(&id).context("failed to read deps")?;
    edges.extend(stores.deps.edges_to(&id).context("failed to read deps")?);
    print_edges(edges);
    Ok(())
}

enum Query {
    Ready,
    Waiting,
    Pipeline,
}

fn cmd_ids_from_query(stores: &Stores, query: Query) -> anyhow::Result<()> {
    let orbs = stores.orbs.load_all().context("failed to read orbs")?;
    let ids = match query {
        Query::Ready => stores.deps.ready(&orbs).context("failed to query ready")?,
        Query::Waiting => stores
            .deps
            .waiting(&orbs)
            .context("failed to query waiting")?,
        Query::Pipeline => stores
            .deps
            .pipeline(&orbs)
            .context("failed to query pipeline")?,
    };
    if ids.is_empty() {
        println!("No orbs");
        return Ok(());
    }
    for id in ids {
        if let Some(orb) = stores.orbs.load_by_id(&id).context("failed to read orb")? {
            println!("{} {}", orb.id, orb.title);
        } else {
            println!("{id}");
        }
    }
    Ok(())
}

fn cmd_tree(stores: &Stores, id: &str) -> anyhow::Result<()> {
    let id = OrbId::from_raw(id);
    let Some(tree) = build_orb_tree(&stores.orbs, &id) else {
        bail!("orb not found: {id}");
    };
    print_tree(&tree);
    Ok(())
}

fn load_orb(stores: &Stores, id: &str) -> anyhow::Result<Orb> {
    let id = OrbId::from_raw(id);
    stores
        .orbs
        .load_by_id(&id)
        .context("failed to read orb")?
        .ok_or_else(|| anyhow::anyhow!("orb not found: {id}"))
}

fn print_edges(edges: Vec<DepEdge>) {
    if edges.is_empty() {
        println!("No deps");
        return;
    }
    for edge in edges {
        println!(
            "{} -{}-> {}",
            edge.from,
            edge_type_label(edge.edge_type),
            edge.to
        );
    }
}

fn print_tree(node: &OrbNode) {
    let indent = "  ".repeat(node.depth);
    println!(
        "{}{} [{}] {}",
        indent,
        node.orb.id,
        lifecycle_label(&node.orb),
        node.orb.title
    );
    for child in &node.children {
        print_tree(child);
    }
}

fn print_orb_summary(orb: &Orb) {
    println!("  title:    {}", orb.title);
    println!("  type:     {}", orb_type_label(&orb.orb_type));
    println!("  state:    {}", lifecycle_label(orb));
    println!(
        "  priority: {} ({})",
        orb.priority,
        priority_name(orb.priority)
    );
    if !orb.labels.is_empty() {
        println!("  labels:   {}", orb.labels.join(", "));
    }
}

fn print_orb_detail(orb: &Orb) {
    println!("Orb:         {}", orb.id);
    println!("Title:       {}", orb.title);
    println!("Description: {}", orb.description);
    println!("Type:        {}", orb_type_label(&orb.orb_type));
    println!("State:       {}", lifecycle_label(orb));
    println!(
        "Priority:    {} ({})",
        orb.priority,
        priority_name(orb.priority)
    );
    if !orb.labels.is_empty() {
        println!("Labels:      {}", orb.labels.join(", "));
    }
    if let Some(parent) = &orb.parent_id {
        println!("Parent:      {parent}");
    }
    if let Some(root) = &orb.root_id {
        println!("Root:        {root}");
    }
    println!("Created:     {}", orb.created_at);
    println!("Updated:     {}", orb.updated_at);
    if let Some(closed_at) = orb.closed_at {
        println!("Closed:      {closed_at}");
    }
    if let Some(result) = &orb.result {
        println!("Result:      {result}");
    }
    if let Some(confidence) = orb.confidence {
        println!("Confidence:  {confidence:.2}");
    }
    if orb.is_tombstoned() {
        println!("Deleted:     yes");
        if let Some(reason) = &orb.delete_reason {
            println!("Reason:      {reason}");
        }
    }
}

fn parse_orb_type(s: &str) -> OrbType {
    match s.to_ascii_lowercase().as_str() {
        "epic" => OrbType::Epic,
        "feature" => OrbType::Feature,
        "task" => OrbType::Task,
        "bug" => OrbType::Bug,
        "chore" => OrbType::Chore,
        "docs" => OrbType::Docs,
        other => OrbType::Custom(other.to_string()),
    }
}

fn parse_edge_type(s: &str) -> anyhow::Result<EdgeType> {
    match s.to_ascii_lowercase().as_str() {
        "blocks" => Ok(EdgeType::Blocks),
        "depends_on" | "depends-on" => Ok(EdgeType::DependsOn),
        "parent" => Ok(EdgeType::Parent),
        "child" => Ok(EdgeType::Child),
        "related" => Ok(EdgeType::Related),
        "duplicates" => Ok(EdgeType::Duplicates),
        "follows" => Ok(EdgeType::Follows),
        other => bail!("unknown edge type: {other}"),
    }
}

#[derive(Clone, Copy)]
enum LifecycleFilter {
    Status(OrbStatus),
    Phase(OrbPhase),
}

fn parse_lifecycle_filter(s: &str) -> anyhow::Result<LifecycleFilter> {
    if let Ok(status) = parse_status(s) {
        return Ok(LifecycleFilter::Status(status));
    }
    Ok(LifecycleFilter::Phase(parse_phase(s)?))
}

fn apply_lifecycle(orb: &mut Orb, s: &str) -> anyhow::Result<()> {
    if orb.orb_type.uses_phase() {
        orb.set_phase(parse_phase(s)?)
            .with_context(|| format!("failed to set phase to {s}"))
    } else {
        orb.set_status(parse_status(s)?)
            .with_context(|| format!("failed to set status to {s}"))
    }
}

fn parse_status(s: &str) -> anyhow::Result<OrbStatus> {
    match s.to_ascii_lowercase().as_str() {
        "draft" => Ok(OrbStatus::Draft),
        "pending" => Ok(OrbStatus::Pending),
        "active" => Ok(OrbStatus::Active),
        "review" => Ok(OrbStatus::Review),
        "done" => Ok(OrbStatus::Done),
        "failed" => Ok(OrbStatus::Failed),
        "cancelled" | "canceled" => Ok(OrbStatus::Cancelled),
        "deferred" => Ok(OrbStatus::Deferred),
        "tombstone" => Ok(OrbStatus::Tombstone),
        other => bail!("unknown status: {other}"),
    }
}

fn parse_phase(s: &str) -> anyhow::Result<OrbPhase> {
    match s.to_ascii_lowercase().as_str() {
        "draft" => Ok(OrbPhase::Draft),
        "pending" => Ok(OrbPhase::Pending),
        "speccing" => Ok(OrbPhase::Speccing),
        "decomposing" => Ok(OrbPhase::Decomposing),
        "refining" => Ok(OrbPhase::Refining),
        "review" => Ok(OrbPhase::Review),
        "waiting" => Ok(OrbPhase::Waiting),
        "executing" => Ok(OrbPhase::Executing),
        "reevaluating" | "re-evaluating" => Ok(OrbPhase::Reevaluating),
        "done" => Ok(OrbPhase::Done),
        "failed" => Ok(OrbPhase::Failed),
        "cancelled" | "canceled" => Ok(OrbPhase::Cancelled),
        "deferred" => Ok(OrbPhase::Deferred),
        "tombstone" => Ok(OrbPhase::Tombstone),
        other => bail!("unknown phase: {other}"),
    }
}

fn lifecycle_matches(orb: &Orb, filter: LifecycleFilter) -> bool {
    match filter {
        LifecycleFilter::Status(status) => {
            orb.status == Some(status)
                || lifecycle_label(orb).eq_ignore_ascii_case(&format!("{status:?}"))
        }
        LifecycleFilter::Phase(phase) => orb.phase == Some(phase),
    }
}

fn lifecycle_label(orb: &Orb) -> String {
    if let Some(status) = orb.status {
        format!("{status:?}").to_ascii_lowercase()
    } else if let Some(phase) = orb.phase {
        format!("{phase:?}").to_ascii_lowercase()
    } else {
        "unknown".to_string()
    }
}

fn orb_type_label(orb_type: &OrbType) -> &str {
    match orb_type {
        OrbType::Epic => "epic",
        OrbType::Feature => "feature",
        OrbType::Task => "task",
        OrbType::Bug => "bug",
        OrbType::Chore => "chore",
        OrbType::Docs => "docs",
        OrbType::Custom(value) => value.as_str(),
    }
}

fn edge_type_label(edge_type: EdgeType) -> &'static str {
    match edge_type {
        EdgeType::Blocks => "blocks",
        EdgeType::DependsOn => "depends_on",
        EdgeType::Parent => "parent",
        EdgeType::Child => "child",
        EdgeType::Related => "related",
        EdgeType::Duplicates => "duplicates",
        EdgeType::Follows => "follows",
    }
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',').map(str::to_string).collect()
}

fn normalize_labels(labels: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    labels
        .into_iter()
        .flat_map(|label| split_csv(&label))
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .filter(|label| seen.insert(label.clone()))
        .collect()
}
