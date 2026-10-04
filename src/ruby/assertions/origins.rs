//! Source-bound callable origins. Resolving a value does not grant assertion trust.
use super::*;

type Namespaces<'a> = BTreeMap<String, (usize, Node<'a>, bool, usize)>;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Callable {
    pub owner: String,
    pub trusted: bool,
    pub proxy: bool,
    pub denied: bool,
    pub negated: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum Value {
    Namespace(String),
    Callable(Callable),
    Export(Callable),
    Tuple(Callable),
    Expectation(Callable),
    Unknown(Callable),
    Copy(Callable),
    Negated(Callable),
}

#[derive(Default, Clone)]
pub(super) struct Origins {
    pub captures: BTreeMap<String, Callable>,
    pub factories: BTreeMap<(usize, usize), Callable>,
    pub dispatch: BTreeSet<(usize, usize)>,
    pub matchers: BTreeSet<(usize, usize)>,
}

impl Origins {
    pub fn collect(
        units: &[super::super::providers::Unit],
        edges: &[SourceDependency],
        configured: &[String],
        registrations: Option<&super::super::providers::Providers>,
        budget: usize,
    ) -> Option<BTreeMap<PathBuf, Self>> {
        if units.iter().any(|unit| unit.tree.root_node().has_error())
            || !super::super::providers::source_graph_acyclic(
                units
                    .iter()
                    .map(|unit| (unit.file.path.as_path(), unit.canonical_path.as_path())),
                edges,
            )
        {
            return Some(BTreeMap::new());
        }
        let mut work = 0usize;
        let mut declarations = BTreeMap::<String, Vec<(usize, Node<'_>)>>::new();
        for (unit, input) in units.iter().enumerate() {
            let nodes = descendants(input.tree.root_node());
            work = work.saturating_add(nodes.len());
            if work > budget {
                return None;
            }
            for module in nodes.into_iter().filter(|node| {
                node.kind() == "module" && node.parent() == Some(input.tree.root_node())
            }) {
                if let Some(name) = module.child_by_field_name("name") {
                    if name.kind() == "constant"
                        && !super::super::ownership::protected_namespace(text(name, &input.source))
                    {
                        declarations
                            .entry(text(name, &input.source).to_owned())
                            .or_default()
                            .push((unit, module));
                    }
                }
            }
        }
        if !declarations.values().any(|owners| {
            owners.len() == 1 && descriptor(owners[0].1, &units[owners[0].0].source).is_some()
        }) {
            return Some(BTreeMap::new());
        }
        let mut invalid = BTreeSet::new();
        let mut largest_provider = 0usize;
        for declarations in declarations.values() {
            for (_, module) in declarations {
                largest_provider = largest_provider.max(descendants(*module).len());
            }
        }
        for input in units {
            let nodes = descendants(input.tree.root_node());
            let mut aliases = BTreeMap::new();
            for node in &nodes {
                if matches!(node.kind(), "assignment" | "operator_assignment") {
                    if let (Some(left), Some(right)) = (
                        node.child_by_field_name("left"),
                        node.child_by_field_name("right"),
                    ) {
                        let name = text(left, &input.source);
                        if declarations.contains_key(name) {
                            invalid.insert(name.to_owned());
                        }
                        for receiver in descendants(left) {
                            if declarations.contains_key(text(receiver, &input.source)) {
                                invalid.insert(text(receiver, &input.source).to_owned());
                            }
                        }
                        let owner = if declarations.contains_key(text(right, &input.source)) {
                            Some(text(right, &input.source).to_owned())
                        } else {
                            aliases.get(text(right, &input.source)).cloned()
                        };
                        if let Some(owner) = owner {
                            aliases.insert(name.to_owned(), owner);
                        }
                    }
                }
            }
            for node in nodes {
                if node.kind() == "call"
                    && call_parts(node, &input.source).is_some_and(|(method, _)| {
                        matches!(
                            method.as_str(),
                            "const_get"
                                | "const_set"
                                | "remove_const"
                                | "define_method"
                                | "define_singleton_method"
                                | "alias_method"
                                | "remove_method"
                                | "undef_method"
                                | "module_eval"
                                | "class_eval"
                                | "instance_eval"
                                | "eval"
                        )
                    })
                {
                    invalid.extend(declarations.keys().cloned());
                }
                if node.kind() == "call" {
                    if let Some(receiver) = node.child_by_field_name("receiver") {
                        let owner = declarations
                            .contains_key(text(receiver, &input.source))
                            .then(|| text(receiver, &input.source).to_owned())
                            .or_else(|| aliases.get(text(receiver, &input.source)).cloned());
                        if let Some(owner) = owner {
                            if !namespace_call(node, &input.source) {
                                invalid.insert(owner);
                            }
                        }
                    }
                }
                let spelling = text(node, &input.source);
                let owner = declarations
                    .contains_key(spelling)
                    .then(|| spelling.to_owned())
                    .or_else(|| aliases.get(spelling).cloned());
                if let Some(owner) = owner {
                    let mut declaration = node;
                    while let Some(parent) = declaration
                        .parent()
                        .filter(|parent| parent.kind() == "scope_resolution")
                    {
                        declaration = parent;
                    }
                    if declaration.parent().is_some_and(|parent| {
                        (matches!(parent.kind(), "class" | "module")
                            && parent.child_by_field_name("name") == Some(declaration)
                            && declaration.kind() == "scope_resolution")
                            || parent.kind() == "singleton_class"
                    }) {
                        invalid.insert(owner.clone());
                    }
                    if node.parent().is_some_and(|parent| {
                        parent.kind() == "argument_list"
                            || (parent.kind() == "singleton_method"
                                && parent.child_by_field_name("object") == Some(node))
                    }) {
                        invalid.insert(owner);
                    }
                }
            }
        }
        let resolution_work = largest_provider.saturating_mul(6).saturating_add(64);
        let mut result = BTreeMap::new();
        let mut summaries = BTreeMap::<usize, (Namespaces<'_>, BTreeMap<String, Value>)>::new();
        let mut remaining: BTreeSet<_> = (0..units.len()).collect();
        let mut order = Vec::new();
        while !remaining.is_empty() {
            work = work.saturating_add(units.len().saturating_mul(edges.len().max(1)));
            if work > budget {
                return None;
            }
            let unit = remaining.iter().copied().find(|index| {
                edges
                    .iter()
                    .filter(|edge| edge.location.path == units[*index].file.path)
                    .all(|edge| {
                        units
                            .iter()
                            .position(|unit| unit.canonical_path == edge.target)
                            .is_none_or(|target| !remaining.contains(&target))
                    })
            })?;
            remaining.remove(&unit);
            order.push(unit);
        }
        let rounds = if units.iter().any(|unit| {
            descendants(unit.tree.root_node()).iter().any(|node| {
                node.kind() == "assignment"
                    && node
                        .child_by_field_name("left")
                        .is_some_and(|left| left.kind() == "constant")
                    && node
                        .parent()
                        .and_then(|body| body.parent())
                        .is_some_and(|owner| owner.kind() == "module")
            })
        }) {
            2
        } else {
            1
        };
        for round in 0..rounds {
            for &unit_id in &order {
                let input = &units[unit_id];
                let root = input.tree.root_node();
                let mut available = BTreeMap::new();
                let mut values = BTreeMap::new();
                let mut initialized = BTreeMap::new();
                let mut exported = BTreeMap::new();
                if round == 1 {
                    work = work.saturating_add(
                        units
                            .len()
                            .saturating_mul(edges.len().max(1))
                            .saturating_mul(32),
                    );
                    if work > budget {
                        return None;
                    }
                    let (namespaces, exports) =
                        preceding_context(unit_id, units, edges, &summaries, 0, &mut work, budget)
                            .unwrap_or_default();
                    available.extend(namespaces);
                    for (name, value) in exports {
                        values.insert(name.clone(), Some(value));
                        initialized.insert(name, 0);
                    }
                }
                let mut origins = Self::default();
                let mut cursor = root.walk();
                for statement in root.named_children(&mut cursor) {
                    if statement.kind() == "call" {
                        for edge in edges.iter().filter(|edge| {
                            edge.location.path == input.file.path
                                && edge.location == location(&input.file, statement, &input.source)
                        }) {
                            let Some(provider) = units
                                .iter()
                                .position(|unit| unit.canonical_path == edge.target)
                            else {
                                continue;
                            };
                            if let Some((namespaces, exports)) = summaries.get(&provider) {
                                for (name, (owner, module, trusted, _)) in namespaces {
                                    available.insert(
                                        name.clone(),
                                        (*owner, *module, *trusted, statement.end_byte()),
                                    );
                                }
                                for (name, value) in exports {
                                    values.insert(name.clone(), Some(value.clone()));
                                    initialized.insert(name.clone(), statement.end_byte());
                                    exported.insert(name.clone(), value.clone());
                                }
                            }
                            for (name, declarations) in &declarations {
                                if !invalid.contains(name)
                                    && declarations.len() == 1
                                    && declarations[0].0 == provider
                                {
                                    let module = declarations[0].1;
                                    work = work.saturating_add(descendants(module).len());
                                    if work > budget {
                                        return None;
                                    }
                                    if descriptor(module, &units[provider].source).is_some() {
                                        available.insert(
                                            name.clone(),
                                            (
                                                provider,
                                                module,
                                                configured_load(
                                                    statement,
                                                    &input.source,
                                                    configured,
                                                ) || configured.iter().any(|item| {
                                                    Path::new(item).is_absolute()
                                                        && Path::new(item) == edge.target
                                                }),
                                                statement.end_byte(),
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if statement.kind() == "module" {
                        if let Some(name) = statement.child_by_field_name("name") {
                            let name = text(name, &input.source);
                            if declarations
                                .get(name)
                                .is_some_and(|owners| owners.len() == 1)
                                && !invalid.contains(name)
                            {
                                if let Some(body) = statement.child_by_field_name("body") {
                                    let mut cursor = body.walk();
                                    let assignments: Vec<_> = body
                                        .named_children(&mut cursor)
                                        .filter(|node| !node.is_extra())
                                        .collect();
                                    let mut constant_names = BTreeSet::new();
                                    if assignments.iter().all(|node| {
                                        node.kind() == "assignment"
                                            && node.child_by_field_name("left").is_some_and(
                                                |left| {
                                                    left.kind() == "constant"
                                                        && constant_names
                                                            .insert(text(left, &input.source))
                                                },
                                            )
                                    }) {
                                        for assignment in assignments {
                                            work = work.saturating_add(resolution_work);
                                            if work > budget {
                                                return None;
                                            }
                                            let left = assignment.child_by_field_name("left")?;
                                            let key =
                                                format!("{name}::{}", text(left, &input.source));
                                            if let Some(value) = resolve(
                                                assignment.child_by_field_name("right")?,
                                                &input.source,
                                                &values,
                                                &available,
                                                units,
                                                0,
                                            ) {
                                                if matches!(value, Value::Callable(ref callable) if !callable.proxy)
                                                {
                                                    exported.insert(key, value);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if statement.kind() == "assignment" {
                        if let (Some(left), Some(right)) = (
                            statement.child_by_field_name("left"),
                            statement.child_by_field_name("right"),
                        ) {
                            let binding = if left.kind() == "identifier" {
                                Some(left)
                            } else if left.kind() == "left_assignment_list"
                                && left.named_child_count() == 1
                            {
                                left.named_child(0)
                            } else {
                                None
                            };
                            if let Some(left) = binding {
                                if values.contains_key(text(left, &input.source)) {
                                    values.insert(text(left, &input.source).to_owned(), None);
                                } else {
                                    work = work.saturating_add(resolution_work);
                                    if work > budget {
                                        return None;
                                    }
                                    let value = resolve(
                                        right,
                                        &input.source,
                                        &values,
                                        &available,
                                        units,
                                        0,
                                    )
                                    .map(|value| {
                                        if let Value::Tuple(ref item) = value {
                                            if left.parent().is_some_and(|parent| {
                                                parent.kind() == "left_assignment_list"
                                            }) {
                                                return Value::Callable(item.clone());
                                            }
                                            value
                                        } else {
                                            value
                                        }
                                    });
                                    values.insert(text(left, &input.source).to_owned(), value);
                                    initialized.insert(
                                        text(left, &input.source).to_owned(),
                                        statement.end_byte(),
                                    );
                                }
                            }
                        }
                    }
                }
                let nodes = descendants(root);
                work = work.saturating_add(
                    nodes.len().saturating_mul(
                        nodes
                            .iter()
                            .filter(|node| root_assignment_call(**node, root))
                            .count()
                            .max(1),
                    ),
                );
                if work > budget {
                    return None;
                }
                let (injected_arguments, injected_factories) =
                    injected_origins(root, &input.source, &values, &available, units);
                // Until a mutation path has a closed identity proof, withdraw every affected
                // callable rather than trusting a stable variable that points at mutable state.
                let mut first_mutation = usize::MAX;
                let escaped_or_mutated = nodes.iter().any(|node| {
                    if node.kind() == "call" {
                        if let Some(receiver) = node.child_by_field_name("receiver") {
                            if let Some(value) =
                                resolve(receiver, &input.source, &values, &available, units, 0)
                            {
                                let method =
                                    call_parts(*node, &input.source).map(|(method, _)| method);
                                let permitted = match value {
                                    Value::Namespace(_) => namespace_call(*node, &input.source),
                                    Value::Expectation(_) => matches!(
                                        method.as_deref(),
                                        Some(
                                            "not"
                                                | "to"
                                                | "to_be"
                                                | "to_equal"
                                                | "equal_to"
                                                | "to_have_class"
                                                | "to_have_value"
                                                | "to_be_visible"
                                        )
                                    ),
                                    Value::Export(_) => {
                                        matches!(method.as_deref(), Some("fetch" | "values_at"))
                                    }
                                    _ => matches!(
                                        method.as_deref(),
                                        Some("call" | "soft" | "not" | "dup" | "object_containing")
                                    ),
                                };
                                if !permitted {
                                    first_mutation = first_mutation.min(node.start_byte());
                                    return true;
                                }
                            }
                        }
                    }
                    if matches!(node.kind(), "assignment" | "operator_assignment") {
                        if let Some(left) = node.child_by_field_name("left") {
                            if left.kind() == "identifier"
                                && node.parent() != Some(root)
                                && values
                                    .get(text(left, &input.source))
                                    .is_some_and(Option::is_some)
                                && !shadowed(left, &input.source)
                            {
                                first_mutation = first_mutation.min(node.start_byte());
                                return true;
                            }
                            if matches!(left.kind(), "call" | "element_reference") {
                                if left.kind() == "call"
                                    && left
                                        .child_by_field_name("method")
                                        .is_some_and(|method| text(method, &input.source) == "not")
                                    && left
                                        .child_by_field_name("receiver")
                                        .and_then(|receiver| {
                                            resolve(
                                                receiver,
                                                &input.source,
                                                &values,
                                                &available,
                                                units,
                                                0,
                                            )
                                        })
                                        .is_some_and(|value| matches!(value, Value::Copy(_)))
                                {
                                    return false;
                                }
                                let changed = descendants(left).iter().any(|part| {
                                    values
                                        .get(text(*part, &input.source))
                                        .is_some_and(Option::is_some)
                                        && !shadowed(*part, &input.source)
                                });
                                if changed {
                                    first_mutation = first_mutation.min(node.start_byte());
                                }
                                return changed;
                            }
                        }
                    }
                    if node.kind() == "identifier"
                        && values
                            .get(text(*node, &input.source))
                            .is_some_and(Option::is_some)
                        && !shadowed(*node, &input.source)
                    {
                        let changed = node.parent().is_some_and(|parent| {
                            (parent.kind() == "argument_list"
                                && !injected_arguments.contains(&node.id()))
                                || (parent.kind() == "assignment"
                                    && parent.child_by_field_name("left") != Some(*node)
                                    && parent.parent() != Some(root))
                        });
                        if changed {
                            first_mutation = first_mutation.min(node.start_byte());
                        }
                        return changed;
                    }
                    false
                });
                if escaped_or_mutated {
                    values.values_mut().for_each(|value| {
                        if let Some(value) = value {
                            deny(value);
                        }
                    });
                }
                for (position, mut callable) in injected_factories {
                    if escaped_or_mutated {
                        callable.denied = true;
                    }
                    origins.factories.insert(position, callable);
                }
                for (name, value) in &values {
                    if let Some(Value::Callable(callable)) = value {
                        origins.captures.insert(name.clone(), callable.clone());
                    } else if let Some(Value::Export(callable)) = value {
                        let mut exported = callable.clone();
                        exported.owner = format!("export:{}", callable.owner);
                        origins.captures.insert(name.clone(), exported);
                    } else if let Some(Value::Namespace(owner)) = value {
                        origins.captures.insert(
                            name.clone(),
                            Callable {
                                owner: format!("namespace:{owner}"),
                                trusted: false,
                                proxy: false,
                                denied: false,
                                negated: false,
                            },
                        );
                    } else if let Some(Value::Negated(callable) | Value::Unknown(callable)) = value
                    {
                        let mut captured = callable.clone();
                        captured.owner = format!("member:{}", captured.owner);
                        if matches!(value, Some(Value::Unknown(_))) {
                            captured.denied = true;
                        }
                        origins.captures.insert(name.clone(), captured);
                    }
                }
                for node in nodes {
                    work = work.saturating_add(1);
                    if work > budget {
                        return None;
                    }
                    if node.kind() != "call" {
                        continue;
                    }
                    let children = descendants(node);
                    work = work
                        .saturating_add(children.len())
                        .saturating_add(resolution_work);
                    if work > budget {
                        return None;
                    }
                    if children.iter().any(|part| {
                        matches!(part.kind(), "identifier" | "scope_resolution")
                            && values.contains_key(text(*part, &input.source))
                            && (initialized
                                .get(text(*part, &input.source))
                                .is_some_and(|position| *position > node.start_byte())
                                || shadowed(*part, &input.source))
                    }) {
                        continue;
                    }
                    let Some(method) = node.child_by_field_name("method") else {
                        continue;
                    };
                    let name = text(method, &input.source);
                    let receiver = node.child_by_field_name("receiver");
                    let callable = receiver
                        .and_then(|receiver| {
                            resolve(receiver, &input.source, &values, &available, units, 0)
                        })
                        .and_then(|value| match value {
                            Value::Callable(callable) | Value::Negated(callable) => Some(callable),
                            _ => None,
                        });
                    if name == "expect" {
                        if let Some(Value::Namespace(owner)) = receiver.and_then(|receiver| {
                            resolve(receiver, &input.source, &values, &available, units, 0)
                        }) {
                            origins.factories.insert(
                                (node.start_byte(), node.end_byte()),
                                Callable {
                                    trusted: available[&owner].2,
                                    owner,
                                    proxy: false,
                                    denied: false,
                                    negated: false,
                                },
                            );
                        }
                    }
                    if name == "object_containing"
                        && callable.as_ref().is_some_and(|callable| {
                            callable.proxy && callable.trusted && !callable.denied
                        })
                    {
                        origins
                            .matchers
                            .insert((node.start_byte(), node.end_byte()));
                    }
                    if name == "call"
                        || (name == "soft"
                            && callable.as_ref().is_some_and(|callable| callable.proxy))
                    {
                        if let Some(callable) = callable.clone() {
                            origins
                                .factories
                                .insert((node.start_byte(), node.end_byte()), callable);
                        }
                    }
                    if matches!(name, "send" | "public_send" | "__send__") {
                        if let Some(callable) = callable {
                            let mut parent = node.parent();
                            let mut immediate = true;
                            while let Some(ancestor) = parent {
                                if matches!(ancestor.kind(), "block" | "do_block" | "lambda" | "method" | "singleton_method" | "class" | "module" | "if" | "unless" | "case" | "while" | "until" | "for" | "rescue" | "ensure") {
                                    immediate = false;
                                    break;
                                }
                                parent = ancestor.parent();
                            }
                            if callable.denied && !(immediate && node.start_byte() < first_mutation) {
                                continue;
                            }
                            let target = call_parts(node, &input.source).map(|(method, _)| method);
                            if target.as_deref() == Some("call")
                                || (callable.proxy
                                    && matches!(target.as_deref(), Some("soft" | "not")))
                            {
                                origins
                                    .dispatch
                                    .insert((node.start_byte(), node.end_byte()));
                                if target.as_deref() != Some("not") {
                                    origins
                                        .factories
                                        .insert((node.start_byte(), node.end_byte()), callable);
                                }
                            }
                        } else if receiver
                            .and_then(|receiver| {
                                resolve(receiver, &input.source, &values, &available, units, 0)
                            })
                            .is_some_and(|value| matches!(value, Value::Expectation(ref callable) if !callable.denied))
                            && call_parts(node, &input.source).is_some_and(|(target, _)| {
                                matches!(
                                    target.as_str(),
                                    "not"
                                        | "to"
                                        | "to_be"
                                        | "to_equal"
                                        | "equal_to"
                                        | "to_have_class"
                                        | "to_have_value"
                                        | "to_be_visible"
                                )
                            })
                        {
                            origins
                                .dispatch
                                .insert((node.start_byte(), node.end_byte()));
                        }
                    }
                    // A bare Method capture is stable only when the registration owner proof has
                    // independently verified its lookup; assertion configuration is not isolation.
                    if matches!(name, "method" | "public_method")
                        && registrations
                            .and_then(|proofs| proofs.get(&input.file.path, &input.source))
                            .is_some_and(|proof| proof.isolated_calls.contains(&node.start_byte()))
                    {
                        origins
                            .dispatch
                            .insert((node.start_byte(), node.end_byte()));
                    }
                }
                // A declaration is exported only when its namespace closes; import timestamps
                // are replaced at each actual preceding load in the consuming file.
                for (name, owners) in &declarations {
                    if owners.len() == 1
                        && owners[0].0 == unit_id
                        && !invalid.contains(name)
                        && descriptor(owners[0].1, &input.source).is_some()
                    {
                        available.insert(name.clone(), (unit_id, owners[0].1, false, 0));
                    }
                }
                summaries.insert(unit_id, (available, exported));
                result.insert(input.file.path.clone(), origins);
            }
        }
        Some(result)
    }
}

type Summary<'a> = (Namespaces<'a>, BTreeMap<String, Value>);

fn preceding_context<'a>(
    target: usize,
    units: &'a [super::super::providers::Unit],
    edges: &[SourceDependency],
    summaries: &BTreeMap<usize, Summary<'a>>,
    depth: usize,
    work: &mut usize,
    budget: usize,
) -> Option<Summary<'a>> {
    *work = work.saturating_add(units.len()).saturating_add(edges.len());
    if depth >= 32 || *work > budget {
        return None;
    }
    let mut contexts = Vec::new();
    for edge in edges
        .iter()
        .filter(|edge| edge.target == units[target].canonical_path)
    {
        let parent = units
            .iter()
            .position(|unit| unit.file.path == edge.location.path)?;
        let root = units[parent].tree.root_node();
        let source = &units[parent].source;
        let nodes = descendants(root);
        *work = work.saturating_add(nodes.len().saturating_mul(edges.len().max(1)));
        if *work > budget {
            return None;
        }
        let load = nodes.iter().find(|node| {
            location(&units[parent].file, **node, source) == edge.location && node.kind() == "call"
        })?;
        if load.parent() != Some(root) {
            return None;
        }
        let (mut namespaces, mut exports) =
            preceding_context(parent, units, edges, summaries, depth + 1, work, budget)?;
        for previous in edges
            .iter()
            .filter(|previous| previous.location.path == units[parent].file.path)
        {
            let Some(call) = nodes.iter().find(|node| {
                node.kind() == "call"
                    && location(&units[parent].file, **node, source) == previous.location
            }) else {
                continue;
            };
            if call.parent() != Some(root) || call.end_byte() >= load.start_byte() {
                continue;
            }
            let provider = units
                .iter()
                .position(|unit| unit.canonical_path == previous.target)?;
            let (loaded, values) = summaries.get(&provider)?;
            namespaces.extend(loaded.iter().map(|(name, (unit, module, trusted, _))| {
                (name.clone(), (*unit, *module, *trusted, 0))
            }));
            exports.extend(values.clone());
        }
        contexts.push((namespaces, exports));
    }
    let Some(mut common) = contexts.pop() else {
        return Some(Default::default());
    };
    for (namespaces, exports) in contexts {
        common
            .0
            .retain(|name, value| namespaces.get(name) == Some(value));
        common
            .1
            .retain(|name, value| exports.get(name) == Some(value));
    }
    Some(common)
}

fn injected_origins(
    root: Node<'_>,
    source: &str,
    values: &BTreeMap<String, Option<Value>>,
    available: &Namespaces<'_>,
    units: &[super::super::providers::Unit],
) -> (BTreeSet<usize>, BTreeMap<(usize, usize), Callable>) {
    let mut arguments = BTreeSet::new();
    let mut factories = BTreeMap::new();
    let nodes = descendants(root);
    for allocation in nodes
        .iter()
        .filter(|node| root_assignment_call(**node, root))
    {
        let Some(receiver) = allocation
            .child_by_field_name("receiver")
            .filter(|node| node.kind() == "constant")
        else {
            continue;
        };
        if allocation.child_by_field_name("block").is_some()
            || allocation
                .child_by_field_name("method")
                .is_none_or(|method| text(method, source) != "new")
        {
            continue;
        }
        let Some(args) = allocation
            .child_by_field_name("arguments")
            .filter(|args| args.named_child_count() == 1)
        else {
            continue;
        };
        let argument = args
            .named_child(0)
            .expect("validated single allocation argument");
        let Some(Value::Callable(callable)) =
            resolve(argument, source, values, available, units, 0)
        else {
            continue;
        };
        let name = text(receiver, source);
        let allocations = units
            .iter()
            .map(|unit| {
                descendants(unit.tree.root_node())
                    .iter()
                    .filter(|node| {
                        node.kind() == "call"
                            && node
                                .child_by_field_name("method")
                                .is_some_and(|method| text(method, &unit.source) == "new")
                            && node
                                .child_by_field_name("receiver")
                                .is_some_and(|receiver| {
                                    text(receiver, &unit.source).trim_start_matches("::") == name
                                })
                    })
                    .count()
            })
            .sum::<usize>();
        if allocations != 1 {
            continue;
        }
        let classes: Vec<_> = nodes
            .iter()
            .filter(|node| {
                node.kind() == "class"
                    && node.parent() == Some(root)
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|node| text(node, source) == name)
            })
            .collect();
        if classes.len() != 1 || classes[0].child_by_field_name("superclass").is_some() {
            continue;
        }
        if units
            .iter()
            .map(|unit| {
                descendants(unit.tree.root_node())
                    .into_iter()
                    .filter(|node| {
                        matches!(node.kind(), "class" | "module")
                            && node
                                .child_by_field_name("name")
                                .is_some_and(|node| text(node, &unit.source) == name)
                    })
                    .count()
            })
            .sum::<usize>()
            != 1
        {
            continue;
        }
        if nodes.iter().any(|node| {
            (matches!(node.kind(), "assignment" | "operator_assignment")
                && node.child_by_field_name("left").is_some_and(|left| {
                    descendants(left)
                        .iter()
                        .any(|node| text(*node, source) == name)
                }))
                || node.kind() == "singleton_method"
        }) {
            continue;
        }
        let Some(body) = classes[0].child_by_field_name("body") else {
            continue;
        };
        let mut cursor = body.walk();
        let methods: Vec<_> = body
            .named_children(&mut cursor)
            .filter(|node| !node.is_extra())
            .collect();
        let mut names = BTreeSet::new();
        if methods.iter().any(|node| {
            node.kind() != "method"
                || node
                    .child_by_field_name("name")
                    .is_none_or(|name| !names.insert(text(name, source)))
        }) {
            continue;
        }
        let Some(initialize) = methods.iter().find(|node| {
            node.child_by_field_name("name")
                .is_some_and(|node| text(node, source) == "initialize")
        }) else {
            continue;
        };
        let Some(parameter) = initialize
            .child_by_field_name("parameters")
            .filter(|params| params.named_child_count() == 1)
            .and_then(|params| params.named_child(0))
            .filter(|node| node.kind() == "identifier")
        else {
            continue;
        };
        let Some(init_body) = initialize
            .child_by_field_name("body")
            .filter(|body| body.named_child_count() == 1)
        else {
            continue;
        };
        let Some(write) = init_body
            .named_child(0)
            .filter(|node| node.kind() == "assignment")
        else {
            continue;
        };
        let Some(field) = write
            .child_by_field_name("left")
            .filter(|node| node.kind() == "instance_variable")
        else {
            continue;
        };
        if write.child_by_field_name("right").is_none_or(|right| {
            right.kind() != "identifier" || text(right, source) != text(parameter, source)
        }) {
            continue;
        }
        let references: Vec<_> = descendants(*classes[0])
            .into_iter()
            .filter(|node| node.kind() == "instance_variable")
            .collect();
        if references.iter().any(|node| {
            *node != field
                && (text(*node, source) != text(field, source)
                    || node.parent().is_none_or(|parent| {
                        parent.kind() != "call"
                            || parent.child_by_field_name("receiver") != Some(*node)
                            || parent
                                .child_by_field_name("method")
                                .is_none_or(|method| text(method, source) != "call")
                            || parent.child_by_field_name("block").is_some()
                            || parent
                                .child_by_field_name("arguments")
                                .is_none_or(|args| args.named_child_count() != 1)
                    }))
        }) {
            continue;
        }
        arguments.insert(argument.id());
        for reference in references.into_iter().filter(|node| *node != field) {
            let call = reference.parent().expect("validated field call");
            factories.insert((call.start_byte(), call.end_byte()), callable.clone());
        }
    }
    (arguments, factories)
}

fn shadowed(node: Node<'_>, source: &str) -> bool {
    let name = text(node, source);
    let mut parent = node.parent();
    while let Some(scope) = parent {
        if matches!(
            scope.kind(),
            "method" | "singleton_method" | "class" | "module" | "singleton_class"
        ) {
            return true;
        }
        if matches!(scope.kind(), "block" | "do_block" | "lambda")
            && scope
                .child_by_field_name("parameters")
                .is_some_and(|parameters| {
                    descendants(parameters).iter().any(|parameter| {
                        parameter.kind() == "identifier" && text(*parameter, source) == name
                    })
                })
        {
            return true;
        }
        parent = scope.parent();
    }
    false
}

fn deny(value: &mut Value) {
    match value {
        Value::Callable(callable)
        | Value::Expectation(callable)
        | Value::Negated(callable)
        | Value::Unknown(callable)
        | Value::Export(callable)
        | Value::Copy(callable)
        | Value::Tuple(callable) => callable.denied = true,
        Value::Namespace(_) => {}
    }
}

fn descriptor<'a>(module: Node<'a>, source: &str) -> Option<Node<'a>> {
    let body = module.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let mut expect = None;
    let mut members = BTreeSet::new();
    for declaration in body
        .named_children(&mut cursor)
        .filter(|node| !node.is_extra())
    {
        let declared_name = text(declaration.child_by_field_name("name")?, source);
        if !members.insert(declared_name) {
            return None;
        }
        if declaration.kind() == "class" && declaration.child_by_field_name("superclass").is_none()
        {
            let mut methods = BTreeSet::new();
            let class_body = declaration.child_by_field_name("body")?;
            let mut cursor = class_body.walk();
            for member in class_body
                .named_children(&mut cursor)
                .filter(|node| !node.is_extra())
            {
                if member.kind() == "method"
                    && !methods.insert(text(member.child_by_field_name("name")?, source))
                {
                    return None;
                }
                if member.kind() == "alias" {
                    let parts: Vec<_> = text(member, source).split_whitespace().collect();
                    if parts.len() != 3 || !methods.insert(parts[1]) {
                        return None;
                    }
                }
                if matches!(
                    member.kind(),
                    "class" | "module" | "singleton_class" | "singleton_method" | "undef"
                ) {
                    return None;
                }
            }
            // Nested classes are not semantic authority; the configured factory export is.
            // Reject lookup/allocation hooks anywhere in the enclosing provider.
            continue;
        }
        if declaration.kind() != "singleton_method"
            || !declaration
                .child_by_field_name("object")
                .is_some_and(|node| node.kind() == "self")
        {
            return None;
        }
        if declared_name == "expect" {
            expect = Some(declaration);
        }
    }
    if descendants(module).iter().any(|node| {
        (matches!(
            node.kind(),
            "method" | "singleton_method" | "class" | "module"
        ) && std::iter::successors(node.parent(), |parent| parent.parent())
            .take_while(|parent| *parent != module)
            .any(|parent| matches!(parent.kind(), "method" | "singleton_method")))
            || matches!(node.kind(), "singleton_class" | "alias" | "undef")
                && node.parent().and_then(|parent| parent.parent()) == Some(module)
            || (matches!(node.kind(), "method" | "singleton_method")
                && node.child_by_field_name("name").is_some_and(|name| {
                    matches!(
                        text(name, source),
                        "new"
                            | "allocate"
                            | "method"
                            | "public_method"
                            | "method_missing"
                            | "const_missing"
                            | "send"
                            | "public_send"
                            | "__send__"
                    )
                }))
            || (reflective_call(*node, source)
                && !(node.kind() == "call"
                    && node.child_by_field_name("receiver").is_none()
                    && node
                        .child_by_field_name("method")
                        .is_some_and(|name| text(name, source) == "method")
                    && expect_argument(node.child_by_field_name("arguments"), source)))
    }) {
        return None;
    }
    expect
}

fn resolve(
    node: Node<'_>,
    source: &str,
    values: &BTreeMap<String, Option<Value>>,
    available: &Namespaces<'_>,
    units: &[super::super::providers::Unit],
    depth: usize,
) -> Option<Value> {
    if depth >= 32 {
        return None;
    }
    match node.kind() {
        "identifier" => values.get(text(node, source)).cloned().flatten(),
        "scope_resolution" => values
            .get(text(node, source).trim_start_matches("::"))
            .cloned()
            .flatten(),
        "constant" => available
            .get(text(node, source))
            .filter(|(_, _, _, loaded)| node.start_byte() >= *loaded)
            .map(|_| Value::Namespace(text(node, source).to_owned())),
        "parenthesized_statements" if node.named_child_count() == 1 => resolve(
            node.named_child(0)?,
            source,
            values,
            available,
            units,
            depth + 1,
        ),
        "element_reference" => {
            let object = node.child_by_field_name("object")?;
            let mut cursor = node.walk();
            let args: Vec<_> = node
                .named_children(&mut cursor)
                .filter(|child| *child != object)
                .collect();
            if args.len() != 1 {
                return None;
            }
            match resolve(object, source, values, available, units, depth + 1)? {
                Value::Export(mut callable) => {
                    if super::super::static_method_name(args[0], source).as_deref()
                        == Some("expect")
                    {
                        Some(Value::Callable(callable))
                    } else {
                        callable.denied = true;
                        Some(Value::Unknown(callable))
                    }
                }
                _ => None,
            }
        }
        "call" => {
            let receiver = node.child_by_field_name("receiver")?;
            let (method, arguments) = call_parts(node, source)?;
            let value = resolve(receiver, source, values, available, units, depth + 1)?;
            let args = node.child_by_field_name("arguments");
            match (value, method.as_str()) {
                (Value::Namespace(owner), "expect") if arguments.len() == 1 => {
                    Some(Value::Expectation(Callable {
                        trusted: available[&owner].2,
                        owner,
                        proxy: false,
                        denied: false,
                        negated: false,
                    }))
                }
                (Value::Callable(callable) | Value::Negated(callable), "call")
                    if arguments.len() == 1 =>
                {
                    Some(Value::Expectation(callable))
                }
                (Value::Callable(callable) | Value::Negated(callable), "soft")
                    if callable.proxy && arguments.len() == 1 =>
                {
                    Some(Value::Expectation(callable))
                }
                (Value::Expectation(callable), "not" | "to") if arguments.is_empty() => {
                    Some(Value::Expectation(callable))
                }
                (Value::Callable(mut callable), "not")
                    if callable.proxy && arguments.is_empty() =>
                {
                    callable.negated = !callable.negated;
                    Some(Value::Negated(callable))
                }
                (Value::Callable(callable), "dup") if callable.proxy && arguments.is_empty() => {
                    Some(Value::Copy(callable))
                }
                (Value::Copy(mut callable), "not") if arguments.is_empty() => {
                    callable.negated = !callable.negated;
                    Some(Value::Negated(callable))
                }
                (Value::Namespace(owner), "method" | "public_method")
                    if expect_argument(args, source) =>
                {
                    Some(Value::Callable(Callable {
                        trusted: available[&owner].2,
                        owner,
                        proxy: false,
                        denied: false,
                        negated: false,
                    }))
                }
                (Value::Namespace(owner), "api") if args.is_none() => {
                    let (unit, module, trusted, _) = available[&owner];
                    let input = &units[unit];
                    let api = descendants(module).into_iter().find(|node| {
                        node.kind() == "singleton_method"
                            && node
                                .child_by_field_name("name")
                                .is_some_and(|name| text(name, &input.source) == "api")
                    })?;
                    let body = api.child_by_field_name("body")?;
                    let hash = body.named_child(0).filter(|node| node.kind() == "hash")?;
                    if body.named_child_count() != 1 || hash.named_child_count() != 1 {
                        return None;
                    }
                    let pair = hash.named_child(0)?;
                    if pair
                        .child_by_field_name("key")
                        .is_none_or(|key| text(key, &input.source) != "expect")
                    {
                        return None;
                    }
                    let constructor = pair.child_by_field_name("value")?;
                    if constructor.kind() != "call"
                        || constructor.child_by_field_name("block").is_some()
                        || constructor
                            .child_by_field_name("arguments")?
                            .named_child_count()
                            != 1
                        || constructor
                            .child_by_field_name("method")
                            .is_none_or(|name| text(name, &input.source) != "new")
                    {
                        return None;
                    }
                    let class = text(constructor.child_by_field_name("receiver")?, &input.source);
                    let capture = constructor
                        .child_by_field_name("arguments")?
                        .named_child(0)?;
                    if capture.kind() != "call"
                        || capture.child_by_field_name("receiver").is_some()
                        || capture.child_by_field_name("block").is_some()
                        || capture
                            .child_by_field_name("arguments")?
                            .named_child_count()
                            != 1
                        || capture
                            .child_by_field_name("method")
                            .is_none_or(|name| text(name, &input.source) != "method")
                        || capture
                            .child_by_field_name("arguments")?
                            .named_child(0)
                            .and_then(|arg| super::super::static_method_name(arg, &input.source))
                            .as_deref()
                            != Some("expect")
                    {
                        return None;
                    }
                    let proxy = descendants(module).into_iter().find(|node| {
                        node.kind() == "class"
                            && node
                                .child_by_field_name("name")
                                .is_some_and(|name| text(name, &input.source) == class)
                    })?;
                    if descendants(proxy).iter().any(|node| {
                        matches!(node.kind(), "method" | "singleton_method")
                            && node.child_by_field_name("name").is_some_and(|name| {
                                matches!(text(name, &input.source), "not" | "dup" | "clone")
                            })
                    }) {
                        return None;
                    }
                    let call = descendants(proxy).into_iter().find(|node| {
                        node.kind() == "method"
                            && node
                                .child_by_field_name("name")
                                .is_some_and(|name| text(name, &input.source) == "call")
                    })?;
                    let body = call.child_by_field_name("body")?;
                    let forward = body.named_child(0)?;
                    if body.named_child_count() != 1
                        || forward.kind() != "call"
                        || forward
                            .child_by_field_name("receiver")
                            .is_none_or(|receiver| receiver.kind() != "instance_variable")
                        || forward
                            .child_by_field_name("method")
                            .is_none_or(|name| text(name, &input.source) != "call")
                    {
                        return None;
                    }
                    let field = text(forward.child_by_field_name("receiver")?, &input.source);
                    let initialize = descendants(proxy).into_iter().find(|node| {
                        node.kind() == "method"
                            && node
                                .child_by_field_name("name")
                                .is_some_and(|name| text(name, &input.source) == "initialize")
                    })?;
                    let parameter = initialize
                        .child_by_field_name("parameters")?
                        .named_child(0)?;
                    let writes: Vec<_> = descendants(proxy)
                        .into_iter()
                        .filter(|node| {
                            node.kind() == "assignment"
                                && node
                                    .child_by_field_name("left")
                                    .is_some_and(|left| text(left, &input.source) == field)
                        })
                        .collect();
                    if parameter.kind() != "identifier"
                        || writes.len() != 1
                        || writes[0].parent() != initialize.child_by_field_name("body")
                        || writes[0].child_by_field_name("right").is_none_or(|right| {
                            text(right, &input.source) != text(parameter, &input.source)
                        })
                    {
                        return None;
                    }
                    if call.child_by_field_name("parameters")?.named_child_count() != 1
                        || forward.child_by_field_name("block").is_some()
                    {
                        return None;
                    }
                    let argument = call.child_by_field_name("parameters")?.named_child(0)?;
                    if argument.kind() != "identifier" {
                        return None;
                    }
                    if forward.child_by_field_name("arguments").is_none_or(|args| {
                        args.named_child_count() != 1
                            || args.named_child(0).is_none_or(|value| {
                                text(value, &input.source) != text(argument, &input.source)
                            })
                    }) {
                        return None;
                    }
                    let soft = descendants(proxy).into_iter().any(|node| {
                        node.kind() == "alias"
                            && text(node, &input.source)
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                == ["alias", "soft", "call"]
                    });
                    if !soft {
                        return None;
                    }
                    Some(Value::Export(Callable {
                        owner,
                        trusted,
                        proxy: true,
                        denied: false,
                        negated: false,
                    }))
                }
                (Value::Export(value), "fetch") if expect_argument(args, source) => {
                    Some(Value::Callable(value))
                }
                (Value::Export(value), "values_at") if expect_argument(args, source) => {
                    Some(Value::Tuple(value))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn namespace_call(node: Node<'_>, source: &str) -> bool {
    call_parts(node, source).is_some_and(|(method, arguments)| {
        matches!(method.as_str(), "expect" | "api")
            || (matches!(method.as_str(), "method" | "public_method")
                && arguments.len() == 1
                && super::super::static_method_name(arguments[0], source).as_deref()
                    == Some("expect"))
    })
}

fn expect_argument(args: Option<Node<'_>>, source: &str) -> bool {
    args.is_some_and(|args| {
        args.named_child_count() == 1
            && args
                .named_child(0)
                .and_then(|arg| super::super::static_method_name(arg, source))
                .as_deref()
                == Some("expect")
    })
}

fn root_assignment_call(node: Node<'_>, root: Node<'_>) -> bool {
    node.kind() == "call"
        && node
            .parent()
            .is_some_and(|parent| parent.kind() == "assignment" && parent.parent() == Some(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_adapter::{SourceFile, SourceLanguage};

    #[test]
    fn origin_work_limits_never_publish_partial_authority() {
        let root = tempfile::tempdir().unwrap();
        let sources = [
            ("provider.rb", "module Assertions; def self.expect(actual); actual; end; end"),
            ("fixture.rb", "require_relative 'provider'; module Fixture; EXPECT = Assertions.method(:expect); end"),
            ("steps.rb", "require_relative 'fixture'; check = Fixture::EXPECT; Given('same') { check.call(value).to_be(1) }"),
        ];
        let files: Vec<_> = sources
            .iter()
            .map(|(name, source)| {
                let path = root.path().join(name);
                std::fs::write(&path, source).unwrap();
                SourceFile {
                    path,
                    language: SourceLanguage::Ruby,
                }
            })
            .collect();
        let units = super::super::super::providers::load_units(&files, (3, 4096))
            .unwrap()
            .unwrap();
        let edges: Vec<_> = [1, 2]
            .into_iter()
            .map(|index| {
                let unit = &units[index];
                SourceDependency::new(
                    location(
                        &unit.file,
                        unit.tree.root_node().named_child(0).unwrap(),
                        &unit.source,
                    ),
                    units[index - 1].canonical_path.clone(),
                )
            })
            .collect();
        let mut exhausted = false;
        let mut complete = false;
        for budget in 0..4000 {
            match Origins::collect(&units, &edges, &["provider".into()], None, budget) {
                None => exhausted = true,
                Some(origins) => {
                    complete = true;
                    assert_eq!(origins[&files[2].path].factories.len(), 1);
                }
            }
        }
        assert!(exhausted && complete);
        let absolute = units[0].canonical_path.to_string_lossy().into_owned();
        let origins = Origins::collect(&units, &edges, &[absolute], None, usize::MAX).unwrap();
        assert!(origins[&files[2].path]
            .factories
            .values()
            .all(|factory| factory.trusted));
        let mut missing_call = edges[1].clone();
        missing_call.location.line = 999;
        missing_call.target = units[0].canonical_path.clone();
        let contexts = preceding_context(
            1,
            &units,
            &[edges[1].clone(), missing_call],
            &BTreeMap::new(),
            0,
            &mut 0,
            usize::MAX,
        )
        .unwrap();
        assert!(contexts.0.is_empty() && contexts.1.is_empty());
        std::fs::write(&files[1].path, "later { require_relative 'provider' }").unwrap();
        let nested = super::super::super::providers::load_units(&files, (3, 4096))
            .unwrap()
            .unwrap();
        let call = descendants(nested[1].tree.root_node())
            .into_iter()
            .find(|node| {
                node.child_by_field_name("method")
                    .is_some_and(|method| text(method, &nested[1].source) == "require_relative")
            })
            .unwrap();
        let nested_edge = SourceDependency::new(
            location(&nested[1].file, call, &nested[1].source),
            nested[0].canonical_path.clone(),
        );
        assert!(preceding_context(
            0,
            &nested,
            &[nested_edge],
            &BTreeMap::new(),
            0,
            &mut 0,
            usize::MAX
        )
        .is_none());
        for (depth, budget) in [(32, usize::MAX), (0, 0), (0, 8)] {
            assert!(
                preceding_context(1, &units, &edges, &BTreeMap::new(), depth, &mut 0, budget)
                    .is_none()
            );
        }
        let node = units[2].tree.root_node().named_child(1).unwrap();
        assert!(resolve(
            node,
            &units[2].source,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &units,
            32
        )
        .is_none());
    }
}
