//! Syntax visitation and lexical binding scopes.
use super::*;

impl<'ast> Visit<'ast> for Collector {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        let qualified = format!("{}::{}", self.lexical_scope(), name(&item.ident));
        let id = self.node(
            qualified.clone(),
            name(&item.ident),
            NodeType::Module,
            "module",
            item.span(),
            DefinitionFlags {
                public: matches!(item.vis, syn::Visibility::Public(_)),
                has_body: item.content.is_some(),
            },
        );
        if let Some(node) = self.result.nodes.iter_mut().find(|node| node.id == id) {
            node.extra.insert("module_path".into(), json!(qualified));
        }
        for attribute in &item.attrs {
            if attribute.path().is_ident("path")
                && let syn::Meta::NameValue(value) = &attribute.meta
                && let syn::Expr::Lit(literal) = &value.value
                && let syn::Lit::Str(path) = &literal.lit
                && let Some(node) = self.result.nodes.iter_mut().find(|node| node.id == id)
            {
                node.extra
                    .insert("module_path_override".into(), json!(path.value()));
            }
        }
        if let Some((_, items)) = &item.content {
            self.module_paths.push(qualified);
            self.parents.push(id);
            let functions = std::mem::take(&mut self.callables);
            let scopes = std::mem::replace(&mut self.scopes, vec![BTreeSet::new()]);
            let owner = self.owner.take();
            let trait_owner = std::mem::replace(&mut self.trait_owner, false);
            for item in items {
                self.visit_item(item);
            }
            self.owner = owner;
            self.trait_owner = trait_owner;
            self.callables = functions;
            self.scopes = scopes;
            self.parents.pop();
            self.module_paths.pop();
        }
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.callable(
            &item.sig,
            Some(&item.block),
            item.span(),
            matches!(item.vis, syn::Visibility::Public(_)),
            "function",
        );
    }
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let old = self.owner.take();
        let old_trait = self.trait_owner;
        let ty = type_name(&item.self_ty);
        let owner = if let Some((_, path, _)) = &item.trait_ {
            format!("<{ty} as {}>", path_name(path))
        } else {
            ty
        };
        let q = format!(
            "{}::<impl {owner}@{}>",
            self.module_path(),
            item.span().start().line
        );
        let id = self.node(
            q,
            format!("impl {owner}"),
            NodeType::Class,
            "impl",
            item.span(),
            DefinitionFlags {
                public: false,
                has_body: true,
            },
        );
        if let Some((_, trait_path, _)) = &item.trait_ {
            let trait_name = path_name(trait_path);
            let target = rust_id(&[
                &self.source,
                "trait-reference",
                &trait_name,
                &item.span().start().line.to_string(),
            ]);
            self.result.nodes.push(GraphNode{id:target.clone(),label:trait_name.clone(),source_file:self.source.clone(),source_location:Some(format!("L{}",item.span().start().line)),node_type:NodeType::Trait,community:None,
                extra:extras(json!({"language":"rust","item_kind":"type_reference","unresolved":true,"has_body":false,"reference_path":trait_name}))});
            self.syntax_edge(
                id.clone(),
                target,
                "implements",
                item.span(),
                "ast:rust-implements",
                HashMap::new(),
            );
        }
        self.owner = Some(owner);
        self.trait_owner = item.trait_.is_some();
        self.parents.push(id);
        self.scopes.push(
            item.generics
                .type_params()
                .map(|p| name(&p.ident))
                .collect(),
        );
        for member in &item.items {
            self.visit_impl_item(member);
        }
        self.scopes.pop();
        self.parents.pop();
        self.owner = old;
        self.trait_owner = old_trait;
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        self.callable(
            &item.sig,
            Some(&item.block),
            item.span(),
            matches!(item.vis, syn::Visibility::Public(_)),
            "method",
        );
    }
    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        let q = format!("{}::{}", self.module_path(), name(&item.ident));
        let id = self.node(
            q,
            name(&item.ident),
            NodeType::Trait,
            "trait",
            item.span(),
            DefinitionFlags {
                public: matches!(item.vis, syn::Visibility::Public(_)),
                has_body: true,
            },
        );
        let old = self.owner.replace(name(&item.ident));
        let old_trait = self.trait_owner;
        self.trait_owner = true;
        self.parents.push(id);
        let names = item
            .generics
            .type_params()
            .map(|p| name(&p.ident))
            .chain(item.generics.const_params().map(|p| name(&p.ident)))
            .collect();
        self.scopes.push(names);
        for member in &item.items {
            self.visit_trait_item(member);
        }
        self.scopes.pop();
        self.parents.pop();
        self.owner = old;
        self.trait_owner = old_trait;
    }
    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        self.callable(
            &item.sig,
            item.default.as_ref(),
            item.span(),
            true,
            "trait_method",
        );
    }
    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        self.node(
            self.qualified(&name(&item.ident)),
            name(&item.ident),
            NodeType::Struct,
            "struct",
            item.span(),
            DefinitionFlags {
                public: matches!(item.vis, syn::Visibility::Public(_)),
                has_body: true,
            },
        );
    }
    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        self.node(
            self.qualified(&name(&item.ident)),
            name(&item.ident),
            NodeType::Enum,
            "enum",
            item.span(),
            DefinitionFlags {
                public: matches!(item.vis, syn::Visibility::Public(_)),
                has_body: true,
            },
        );
    }
    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        self.node(
            self.qualified(&name(&item.ident)),
            name(&item.ident),
            NodeType::Constant,
            "constant",
            item.span(),
            DefinitionFlags {
                public: matches!(item.vis, syn::Visibility::Public(_)),
                has_body: true,
            },
        );
    }
    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        self.node(
            self.qualified(&name(&item.ident)),
            name(&item.ident),
            NodeType::Variable,
            "static",
            item.span(),
            DefinitionFlags {
                public: matches!(item.vis, syn::Visibility::Public(_)),
                has_body: true,
            },
        );
    }
    fn visit_item_type(&mut self, item: &'ast syn::ItemType) {
        self.node(
            self.qualified(&name(&item.ident)),
            name(&item.ident),
            NodeType::Struct,
            "type_alias",
            item.span(),
            DefinitionFlags {
                public: matches!(item.vis, syn::Visibility::Public(_)),
                has_body: true,
            },
        );
        if !self.callables.is_empty() {
            self.scopes
                .last_mut()
                .expect("scope")
                .insert(name(&item.ident));
        }
    }
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        let prefix = if item.leading_colon.is_some() {
            "::"
        } else {
            ""
        };
        self.use_tree(
            prefix,
            &item.tree,
            matches!(item.vis, syn::Visibility::Public(_)),
            item.span(),
        );
    }
    fn visit_block(&mut self, block: &'ast syn::Block) {
        let mut names = BTreeSet::new();
        for statement in &block.stmts {
            if let syn::Stmt::Item(item) = statement {
                let ident = match item {
                    syn::Item::Type(i) => Some(&i.ident),
                    syn::Item::Const(i) => Some(&i.ident),
                    syn::Item::Static(i) => Some(&i.ident),
                    syn::Item::Struct(i) => Some(&i.ident),
                    syn::Item::Enum(i) => Some(&i.ident),
                    _ => None,
                };
                if let Some(ident) = ident {
                    names.insert(name(ident));
                }
            }
        }
        self.scopes.push(names);
        self.blocks.push(block.span());
        visit::visit_block(self, block);
        self.blocks.pop();
        self.scopes.pop();
    }
    fn visit_local(&mut self, item: &'ast syn::Local) {
        if let Some(init) = &item.init {
            self.visit_expr(&init.expr);
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
        }
        self.scopes
            .last_mut()
            .expect("scope")
            .extend(bindings(&item.pat));
    }
    fn visit_expr_call(&mut self, item: &'ast syn::ExprCall) {
        let mut function = &*item.func;
        while let syn::Expr::Paren(parenthesized) = function {
            function = &parenthesized.expr;
        }
        if let syn::Expr::Path(path) = function {
            self.call(
                path_name(&path.path),
                if path.qself.is_some() { "ufcs" } else { "path" },
                item.span(),
            );
        } else {
            self.call(
                function.to_token_stream().to_string(),
                "dynamic",
                item.span(),
            );
        }
        visit::visit_expr_call(self, item);
    }
    fn visit_expr_method_call(&mut self, item: &'ast syn::ExprMethodCall) {
        self.call(format!(".{}", name(&item.method)), "method", item.span());
        visit::visit_expr_method_call(self, item);
    }
    fn visit_stmt_macro(&mut self, item: &'ast syn::StmtMacro) {
        self.call(
            format!("{}!", path_name(&item.mac.path)),
            "macro",
            item.span(),
        );
    }
    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if item.ident.is_none() {
            self.call(
                format!("{}!", path_name(&item.mac.path)),
                "macro",
                item.span(),
            );
        }
    }
    fn visit_expr_macro(&mut self, item: &'ast syn::ExprMacro) {
        self.call(
            format!("{}!", path_name(&item.mac.path)),
            "macro",
            item.span(),
        );
    }
    fn visit_expr_closure(&mut self, item: &'ast syn::ExprClosure) {
        let label = format!(
            "<closure@{}:{}>",
            item.span().start().line,
            item.span().start().column + 1
        );
        let q = format!("{}::{label}", self.lexical_scope());
        let id = self.node(
            q.clone(),
            label,
            NodeType::Function,
            "closure",
            item.span(),
            DefinitionFlags {
                public: false,
                has_body: true,
            },
        );
        let initial = item.inputs.iter().flat_map(bindings).collect();
        self.enter_callable(id, q, initial);
        self.visit_expr(&item.body);
        self.leave_callable();
    }
    fn visit_expr_async(&mut self, item: &'ast syn::ExprAsync) {
        let label = format!(
            "<async@{}:{}>",
            item.span().start().line,
            item.span().start().column + 1
        );
        let q = format!("{}::{label}", self.lexical_scope());
        let id = self.node(
            q.clone(),
            label,
            NodeType::Function,
            "async_block",
            item.span(),
            DefinitionFlags {
                public: false,
                has_body: true,
            },
        );
        self.enter_callable(id, q, BTreeSet::new());
        self.visit_block(&item.block);
        self.leave_callable();
    }
    fn visit_expr_for_loop(&mut self, item: &'ast syn::ExprForLoop) {
        self.visit_expr(&item.expr);
        self.scopes.push(bindings(&item.pat));
        self.visit_block(&item.body);
        self.scopes.pop();
    }
    fn visit_arm(&mut self, item: &'ast syn::Arm) {
        self.scopes.push(bindings(&item.pat));
        if let Some((_, guard)) = &item.guard {
            self.visit_expr(guard);
        }
        self.visit_expr(&item.body);
        self.scopes.pop();
    }
    fn visit_expr_if(&mut self, item: &'ast syn::ExprIf) {
        self.scoped_condition(&item.cond, &item.then_branch);
        if let Some((_, branch)) = &item.else_branch {
            self.visit_expr(branch);
        }
    }
    fn visit_expr_while(&mut self, item: &'ast syn::ExprWhile) {
        self.scoped_condition(&item.cond, &item.body);
    }
}
