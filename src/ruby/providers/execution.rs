//! Single-invocation local callable registration discovery; captured scopes stay uncertain.
use super::*;

impl Graph<'_> {
    pub(super) fn closed_execution(&self, result: &mut Providers) {
        for input in self.units {
            let root = input.tree.root_node();
            if root.has_error() {
                continue;
            }
            let nodes = descendants(root);
            let references = identifier_references(&nodes, &input.source);
            for (assignment, left, value) in root_assignments(&nodes, root) {
                if left.kind() != "identifier" {
                    continue;
                }
                let block = if value.kind() == "lambda" {
                    value.child_by_field_name("body")
                } else if value.kind() == "call"
                    && value.child_by_field_name("receiver").is_none()
                    && value
                        .child_by_field_name("method")
                        .is_some_and(|n| matches!(text(n, &input.source), "lambda" | "proc"))
                    && value
                        .child_by_field_name("arguments")
                        .is_none_or(|n| n.named_child_count() == 0)
                {
                    value.child_by_field_name("block")
                } else {
                    None
                };
                let Some(block) = block else { continue };
                let parameters = value
                    .child_by_field_name("parameters")
                    .or_else(|| block.child_by_field_name("parameters"));
                if parameters.is_some_and(|params| {
                    (0..params.named_child_count()).any(|i| {
                        params
                            .named_child(i as u32)
                            .is_some_and(|n| n.kind() != "identifier")
                    })
                }) {
                    continue;
                }
                let Some(refs) = references.get(text(left, &input.source)) else {
                    continue;
                };
                let uses: Vec<_> = refs.iter().filter(|n| **n != left).collect();
                if uses.len() != 1 {
                    continue;
                }
                let Some(invocation) = closed_call(*uses[0], assignment, &input.source) else {
                    continue;
                };
                let arguments = invocation.child_by_field_name("arguments");
                if arguments.is_some_and(|args| {
                    descendants(args)
                        .iter()
                        .any(|n| n.kind() == "interpolation")
                        || (0..args.named_child_count()).any(|i| {
                            args.named_child(i as u32).is_some_and(|n| {
                                !matches!(
                                    n.kind(),
                                    "integer"
                                        | "float"
                                        | "simple_symbol"
                                        | "string"
                                        | "true"
                                        | "false"
                                        | "nil"
                                )
                            })
                        })
                }) {
                    continue;
                }
                let strict = value.kind() == "lambda"
                    || value
                        .child_by_field_name("method")
                        .is_some_and(|n| text(n, &input.source) == "lambda");
                let arity = parameters.map_or(0, |params| {
                    (0..params.named_child_count())
                        .filter(|i| params.field_name_for_named_child(*i as u32) != Some("locals"))
                        .count()
                });
                if strict && arguments.map_or(0, |args| args.named_child_count()) != arity {
                    continue;
                }
                let body = block.child_by_field_name("body").unwrap_or(block);
                let mut cursor = body.walk();
                let statements: Vec<_> = body
                    .named_children(&mut cursor)
                    .filter(|n| !n.is_extra())
                    .collect();
                let mut registrations = Vec::new();
                let closed = statements.iter().all(|statement| {
                    if statement.kind() == "assignment" {
                        return statement
                            .child_by_field_name("left")
                            .is_some_and(|n| n.kind() == "identifier")
                            && statement.child_by_field_name("right").is_some_and(|n| {
                                matches!(
                                    n.kind(),
                                    "integer"
                                        | "float"
                                        | "simple_symbol"
                                        | "true"
                                        | "false"
                                        | "nil"
                                )
                            });
                    }
                    if statement.kind() != "call"
                        || statement.child_by_field_name("receiver").is_some()
                        || !statement
                            .child_by_field_name("method")
                            .is_some_and(|n| registration(text(n, &input.source)))
                    {
                        return false;
                    }
                    if !statement
                        .child_by_field_name("arguments")
                        .is_some_and(|args| args.named_child_count() == 1)
                    {
                        return false;
                    }
                    let Some(block) = statement.child_by_field_name("block") else {
                        return false;
                    };
                    let mut handler =
                        super::super::handler::fingerprint(block, root, &input.source, None);
                    handler.comparable = false;
                    registrations.push((statement.start_byte(), handler));
                    true
                });
                if closed && !registrations.is_empty() {
                    let proof = result.0.entry(input.file.path.clone()).or_default();
                    for (offset, handler) in registrations {
                        proof.executed.insert(offset);
                        proof.handlers.insert(offset, handler);
                    }
                }
            }
        }
    }
}
