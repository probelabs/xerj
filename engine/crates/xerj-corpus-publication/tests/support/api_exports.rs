use std::collections::BTreeSet;

use syn::{ForeignItem, Item, UseTree, Visibility};

pub fn audit_exact_crate_root_exports(source: &str, allowlist: &[&str]) -> Result<(), String> {
    if !allowlist.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err("crate-root export allowlist must be strictly sorted and unique".into());
    }
    let actual = crate_root_exports(source)?;
    let expected = allowlist
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    if actual == expected {
        return Ok(());
    }
    let actual_set = actual.iter().cloned().collect::<BTreeSet<_>>();
    let expected_set = expected.iter().cloned().collect::<BTreeSet<_>>();
    let unexpected = actual_set
        .difference(&expected_set)
        .cloned()
        .collect::<Vec<_>>();
    let missing = expected_set
        .difference(&actual_set)
        .cloned()
        .collect::<Vec<_>>();
    Err(format!(
        "crate-root export ledger mismatch; unexpected={unexpected:?}; missing={missing:?}"
    ))
}

pub fn crate_root_exports(source: &str) -> Result<Vec<String>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("cannot parse crate root: {error}"))?;
    let mut exports = BTreeSet::new();
    for item in &file.items {
        collect_item(item, &mut exports)?;
    }
    Ok(exports.into_iter().collect())
}

fn collect_item(item: &Item, exports: &mut BTreeSet<String>) -> Result<(), String> {
    match item {
        Item::Use(item) if is_public(&item.vis) => {
            collect_use_tree(&item.tree, None, exports)?;
        }
        Item::Const(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::Enum(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::ExternCrate(item) if is_public(&item.vis) => {
            let exported = item
                .rename
                .as_ref()
                .map_or_else(|| item.ident.to_string(), |(_, alias)| alias.to_string());
            insert(&exported, exports)?;
        }
        Item::Fn(item) if is_public(&item.vis) => insert(&item.sig.ident.to_string(), exports)?,
        Item::ForeignMod(item) => {
            for foreign in &item.items {
                match foreign {
                    ForeignItem::Fn(item) if is_public(&item.vis) => {
                        insert(&item.sig.ident.to_string(), exports)?;
                    }
                    ForeignItem::Static(item) if is_public(&item.vis) => {
                        insert(&item.ident.to_string(), exports)?;
                    }
                    ForeignItem::Type(item) if is_public(&item.vis) => {
                        insert(&item.ident.to_string(), exports)?;
                    }
                    ForeignItem::Macro(_) | ForeignItem::Verbatim(_) => {
                        return Err(
                            "unsupported foreign item could conceal a crate-root export".into()
                        );
                    }
                    _ => {}
                }
            }
        }
        Item::Macro(item) if has_attribute(&item.attrs, "macro_export") => {
            let name = item.ident.as_ref().ok_or_else(|| {
                "#[macro_export] item has no statically auditable exported name".to_owned()
            })?;
            insert(&name.to_string(), exports)?;
        }
        Item::Macro(item) if !item.mac.path.is_ident("macro_rules") => {
            return Err(format!(
                "crate-root macro invocation `{}` could synthesize an unaudited public export",
                path_string(&item.mac.path)
            ));
        }
        Item::Mod(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::Static(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::Struct(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::Trait(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::TraitAlias(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::Type(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::Union(item) if is_public(&item.vis) => insert(&item.ident.to_string(), exports)?,
        Item::Verbatim(tokens) if tokens.to_string().contains("pub") => {
            return Err("unparsed public crate-root tokens cannot be audited".into());
        }
        _ => {}
    }
    Ok(())
}

fn collect_use_tree(
    tree: &UseTree,
    path_tail: Option<&str>,
    exports: &mut BTreeSet<String>,
) -> Result<(), String> {
    match tree {
        UseTree::Path(path) => {
            let segment = path.ident.to_string();
            collect_use_tree(&path.tree, Some(&segment), exports)
        }
        UseTree::Name(name) if name.ident == "self" => {
            let exported =
                path_tail.ok_or_else(|| "public `use self` has no export name".to_owned())?;
            insert(exported, exports)
        }
        UseTree::Name(name) => insert(&name.ident.to_string(), exports),
        UseTree::Rename(rename) if rename.rename == "_" => {
            Err("anonymous public re-exports are not permitted by the API ledger".into())
        }
        UseTree::Rename(rename) => insert(&rename.rename.to_string(), exports),
        UseTree::Group(group) => {
            for child in &group.items {
                collect_use_tree(child, path_tail, exports)?;
            }
            Ok(())
        }
        UseTree::Glob(_) => {
            Err("public glob re-exports cannot be checked against an exact allowlist".into())
        }
    }
}

fn is_public(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public(_))
}

fn has_attribute(attributes: &[syn::Attribute], name: &str) -> bool {
    attributes
        .iter()
        .any(|attribute| attribute.path().is_ident(name))
}

fn insert(name: &str, exports: &mut BTreeSet<String>) -> Result<(), String> {
    if exports.insert(name.to_owned()) {
        Ok(())
    } else {
        Err(format!("duplicate crate-root export `{name}`"))
    }
}

fn path_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}
