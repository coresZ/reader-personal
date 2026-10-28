//! `let` 重声明形参的兼容补丁回归。
//!
//! Rhino（legado 手机端）允许 `let` 与同函数形参同名，正式 ES 规范禁止。
//! 书源里这种写法很常见（给形参一个默认值），QuickJS 严格照规范报
//! `invalid redefinition of parameter name`，整个 jsLib 解析失败 ——
//! 症状是登录表单弹不出来、发现页空白。
use reader_rust::parser::js::{eval_js, patch_let_redeclared_params_for_test};

/// 补丁必须把「与形参同名的 `let`」改成 `var`，脚本因此可解析且语义不变。
#[test]
fn let_shadowing_param_is_rewritten_to_var() {
    let src = r#"
function GetUrl(path, params, sourceUrl) {
  let sourceUrl = sourceUrl || "https://example.test";
  return sourceUrl;
}
"#;

    fn parses(src: &str) -> bool {
        let runtime = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&runtime).unwrap();
        ctx.with(|c| {
            // 包进函数体只做解析、不执行，与 jsLib 的真实形态一致。
            c.eval::<rquickjs::Value, _>(format!("(function () {{\n{src}\n}})"))
                .is_ok()
        })
    }

    assert!(!parses(src), "let 与形参同名时应解析失败");

    let patched = patch_let_redeclared_params_for_test(src).expect("应发生改写");
    assert!(
        patched.contains("var sourceUrl = sourceUrl"),
        "应把 let 改成 var，实际：{patched}"
    );
    assert!(parses(&patched), "改写后应可解析：{patched}");

    // 语义不变：仍能给形参兜默认值。
    let out = eval_js(&format!("{patched}\n;GetUrl('', '', '')"), "", "").expect("应可执行");
    assert_eq!(out, "https://example.test");
}

/// 不能误改：块内的同名 `let` 与外层形参无关时要保留块作用域语义。
#[test]
fn ordinary_let_is_untouched() {
    let src = r#"
function g() {
  let x = 1;
  { let x = 2; }
  return x;
}
"#;
    assert!(
        patch_let_redeclared_params_for_test(src).is_none(),
        "普通 let 不应被改写"
    );
    let out = eval_js(&format!("{src}\n;g()"), "", "").expect("应可执行");
    assert_eq!(out, "1", "块内 let 不应泄漏到函数作用域");
}

/// 没有任何形参同名时，补丁整体不触发（避免无谓改写）。
#[test]
fn no_patch_when_nothing_shadows() {
    let src = "function f(a, b) { let c = a + b; return c; }";
    assert!(patch_let_redeclared_params_for_test(src).is_none());
}
