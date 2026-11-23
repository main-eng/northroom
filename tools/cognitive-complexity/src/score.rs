//! Cognitive complexity, as `SonarSource` defines it (G. Ann Campbell, "Cognitive Complexity",
//! 2018), applied to Rust.
//!
//! - `if`, `match`, `for`, `while` and `loop` add one plus the depth they are nested at.
//! - `else if`, `else` and the `else` of a `let … else` add one, whatever the depth.
//! - Each run of like boolean operators adds one: `a && b && c` is one, `a && b || c` two.
//! - A `break` or `continue` to a label adds one.
//! - Branch bodies, match arms and closures are one level deeper.
//!
//! Nested `fn` items are scored on their own. Macro bodies are scored when they parse as a
//! comma-separated list of expressions (`assert!`, `vec!`, `format!`) or as statements.

use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{
    Attribute, BinOp, Block, Expr, ExprBinary, ExprBreak, ExprClosure, ExprContinue, ExprForLoop,
    ExprIf, ExprLoop, ExprMatch, ExprWhile, ImplItemFn, Item, ItemImpl, ItemMod, ItemTrait, Local,
    Macro, Token, TraitItemFn,
};

/// One function and its score.
#[derive(Debug, PartialEq, Eq)]
pub struct FunctionScore {
    pub name: String,
    pub line: usize,
    pub score: u32,
}

/// Scores every function with a body in one Rust source file.
pub fn score_file(source: &str) -> syn::Result<Vec<FunctionScore>> {
    let file = syn::parse_file(source)?;
    let mut functions = Functions::default();
    functions.visit_file(&file);
    Ok(functions.scores)
}

/// Walks items, scoring each function body; `owner` prefixes methods with their type or trait.
#[derive(Default)]
struct Functions {
    scores: Vec<FunctionScore>,
    owner: String,
}

impl Functions {
    fn score(&mut self, ident: &syn::Ident, block: &Block) {
        let mut body = Body {
            score: 0,
            depth: 0,
            nested_items: Vec::new(),
        };
        body.visit_block(block);
        self.scores.push(FunctionScore {
            name: format!("{}{ident}", self.owner),
            line: ident.span().start().line,
            score: body.score,
        });
        for item in body.nested_items {
            let mut nested = Functions::default();
            nested.visit_item(&item);
            self.scores.append(&mut nested.scores);
        }
    }

    fn with_owner(&mut self, owner: String, walk: impl FnOnce(&mut Self)) {
        let outer = std::mem::replace(&mut self.owner, owner);
        walk(self);
        self.owner = outer;
    }
}

impl<'ast> Visit<'ast> for Functions {
    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.score(&function.sig.ident, &function.block);
    }

    fn visit_item_impl(&mut self, block: &'ast ItemImpl) {
        let self_type = &block.self_ty;
        let owner = format!("{}::", quote::quote!(#self_type)).replace(' ', "");
        self.with_owner(owner, |functions| visit::visit_item_impl(functions, block));
    }

    fn visit_impl_item_fn(&mut self, function: &'ast ImplItemFn) {
        self.score(&function.sig.ident, &function.block);
    }

    fn visit_item_trait(&mut self, block: &'ast ItemTrait) {
        let owner = format!("{}::", block.ident);
        self.with_owner(owner, |functions| visit::visit_item_trait(functions, block));
    }

    fn visit_trait_item_fn(&mut self, function: &'ast TraitItemFn) {
        if let Some(body) = &function.default {
            self.score(&function.sig.ident, body);
        }
    }

    fn visit_item_mod(&mut self, module: &'ast ItemMod) {
        self.with_owner(String::new(), |functions| {
            visit::visit_item_mod(functions, module);
        });
    }

    fn visit_attribute(&mut self, _: &'ast Attribute) {}
}

/// Scores one function body. Items declared inside it are set aside to be scored on their own.
struct Body {
    score: u32,
    depth: u32,
    nested_items: Vec<Item>,
}

impl Body {
    fn structural(&mut self) {
        self.score += 1 + self.depth;
    }

    fn deeper(&mut self, walk: impl FnOnce(&mut Self)) {
        self.depth += 1;
        walk(self);
        self.depth -= 1;
    }

    fn else_branch(&mut self, branch: &Expr) {
        self.score += 1;
        match branch {
            Expr::If(else_if) => self.if_chain(else_if),
            other => self.deeper(|body| body.visit_expr(other)),
        }
    }

    fn if_chain(&mut self, expr: &ExprIf) {
        self.visit_expr(&expr.cond);
        self.deeper(|body| body.visit_block(&expr.then_branch));
        if let Some((_, branch)) = &expr.else_branch {
            self.else_branch(branch);
        }
    }

    fn macro_body(&mut self, mac: &Macro) {
        let as_list = Punctuated::<Expr, Token![,]>::parse_terminated.parse2(mac.tokens.clone());
        if let Ok(list) = as_list {
            list.iter().for_each(|expr| self.visit_expr(expr));
        } else if let Ok(statements) = Block::parse_within.parse2(mac.tokens.clone()) {
            for statement in &statements {
                self.visit_stmt(statement);
            }
        }
    }
}

/// `true` for `&&`, `false` for `||`, `None` for every other operator.
fn boolean_operator(op: &BinOp) -> Option<bool> {
    match op {
        BinOp::And(_) => Some(true),
        BinOp::Or(_) => Some(false),
        _ => None,
    }
}

/// Flattens a chain of `&&`/`||` (through parentheses) into its operators and its operands.
fn flatten_boolean_chain<'e>(
    expr: &'e Expr,
    operators: &mut Vec<bool>,
    operands: &mut Vec<&'e Expr>,
) {
    match expr {
        Expr::Paren(paren) => flatten_boolean_chain(&paren.expr, operators, operands),
        Expr::Binary(binary) => match boolean_operator(&binary.op) {
            Some(operator) => {
                flatten_boolean_chain(&binary.left, operators, operands);
                operators.push(operator);
                flatten_boolean_chain(&binary.right, operators, operands);
            }
            None => operands.push(expr),
        },
        other => operands.push(other),
    }
}

impl<'ast> Visit<'ast> for Body {
    fn visit_expr_if(&mut self, expr: &'ast ExprIf) {
        self.structural();
        self.if_chain(expr);
    }

    fn visit_expr_match(&mut self, expr: &'ast ExprMatch) {
        self.structural();
        self.visit_expr(&expr.expr);
        self.deeper(|body| {
            for arm in &expr.arms {
                if let Some((_, guard)) = &arm.guard {
                    body.visit_expr(guard);
                }
                body.visit_expr(&arm.body);
            }
        });
    }

    fn visit_expr_for_loop(&mut self, expr: &'ast ExprForLoop) {
        self.structural();
        self.visit_expr(&expr.expr);
        self.deeper(|body| body.visit_block(&expr.body));
    }

    fn visit_expr_while(&mut self, expr: &'ast ExprWhile) {
        self.structural();
        self.visit_expr(&expr.cond);
        self.deeper(|body| body.visit_block(&expr.body));
    }

    fn visit_expr_loop(&mut self, expr: &'ast ExprLoop) {
        self.structural();
        self.deeper(|body| body.visit_block(&expr.body));
    }

    fn visit_expr_closure(&mut self, expr: &'ast ExprClosure) {
        self.deeper(|body| body.visit_expr(&expr.body));
    }

    fn visit_local(&mut self, local: &'ast Local) {
        let Some(init) = &local.init else { return };
        self.visit_expr(&init.expr);
        if let Some((_, diverge)) = &init.diverge {
            self.score += 1;
            self.deeper(|body| body.visit_expr(diverge));
        }
    }

    fn visit_expr_break(&mut self, expr: &'ast ExprBreak) {
        if expr.label.is_some() {
            self.score += 1;
        }
        if let Some(value) = &expr.expr {
            self.visit_expr(value);
        }
    }

    fn visit_expr_continue(&mut self, expr: &'ast ExprContinue) {
        if expr.label.is_some() {
            self.score += 1;
        }
    }

    fn visit_expr_binary(&mut self, expr: &'ast ExprBinary) {
        if boolean_operator(&expr.op).is_none() {
            return visit::visit_expr_binary(self, expr);
        }
        let whole = Expr::Binary(expr.clone());
        let (mut operators, mut operands) = (Vec::new(), Vec::new());
        flatten_boolean_chain(&whole, &mut operators, &mut operands);
        let changes = operators
            .windows(2)
            .filter(|pair| pair[0] != pair[1])
            .count();
        self.score +=
            1 + u32::try_from(changes).expect("an expression has fewer than 2^32 operators");
        for operand in operands {
            self.visit_expr(operand);
        }
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        self.macro_body(mac);
    }

    fn visit_item(&mut self, item: &'ast Item) {
        self.nested_items.push(item.clone());
    }

    fn visit_attribute(&mut self, _: &'ast Attribute) {}
}

#[cfg(test)]
mod tests {
    use super::score_file;

    fn scores(source: &str) -> Vec<(String, u32)> {
        score_file(source)
            .expect("test source parses")
            .into_iter()
            .map(|function| (function.name, function.score))
            .collect()
    }

    #[test]
    fn scores_sonar_reference_examples() {
        let source = r#"
            fn sum_of_primes(max: i32) -> i32 {
                let mut total = 0;
                'out: for i in 1..=max {
                    for j in 2..i {
                        if i % j == 0 { continue 'out; }
                    }
                    total += i;
                }
                total
            }
            fn get_words(n: i32) -> &'static str {
                match n { 1 => "one", 2 => "a couple", _ => "lots" }
            }
        "#;
        assert_eq!(
            scores(source),
            [("sum_of_primes".into(), 7), ("get_words".into(), 1)]
        );
    }

    #[test]
    fn counts_each_run_of_like_boolean_operators_once() {
        let source = "fn f(a: bool, b: bool, c: bool) -> bool { (a && b && c) || (a && (b || c)) }";
        // `&& && || && ||` flattens to four runs.
        assert_eq!(scores(source), [("f".into(), 4)]);
    }

    #[test]
    fn else_branches_add_one_without_nesting() {
        let source = "fn f(a: bool, b: bool) -> u8 { if a { 1 } else if b { 2 } else { 3 } }";
        assert_eq!(scores(source), [("f".into(), 3)]);
    }

    #[test]
    fn closures_and_macro_bodies_count_at_their_depth() {
        let source =
            "fn f(v: &[i32]) { v.iter().for_each(|x| if *x > 0 { assert!(*x > 1 && *x < 9); }); }";
        assert_eq!(scores(source), [("f".into(), 3)]);
    }

    #[test]
    fn let_else_adds_one() {
        let source = "fn f(v: Option<u8>) -> u8 { let Some(x) = v else { return 0 }; x }";
        assert_eq!(scores(source), [("f".into(), 1)]);
    }

    #[test]
    fn nested_functions_are_scored_on_their_own() {
        let source = "impl S { fn outer(&self) { fn inner(a: bool) { if a {} } if true {} } }";
        assert_eq!(
            scores(source),
            [("S::outer".into(), 1), ("inner".into(), 1)]
        );
    }
}
