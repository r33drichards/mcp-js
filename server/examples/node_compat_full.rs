#[path = "node_compat_full/result.rs"]
mod result;
#[path = "node_compat_full/shard.rs"]
mod shard;
use deno_core::ModuleSpecifier;
use result::{BroadResult, ResultStatus, ShardSummary};
use serde::Deserialize;
use server::engine::{
    CompileMode, ExecutionConfig,
    console::ProcessExitState,
    fetch::FetchConfig,
    fs::FsConfig,
    module_loader::ModuleLoaderConfig,
    opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind},
    subprocess::SubprocessConfig,
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Once,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const PRELUDE: &str = include_str!("../tests/node_compat/runner/prelude.js");
const SENTINEL: &str = "__NODE_TEST_RESULT__";
static INIT: Once = Once::new();

#[derive(Debug)]
struct NodeCliInvocation {
    environment_requires: Vec<String>,
    environment_imports: Vec<String>,
    cli_requires: Vec<String>,
    cli_imports: Vec<String>,
    experimental_loaders: Vec<String>,
    exec_argv: Vec<String>,
    eval_source: Option<String>,
    input_type: Option<String>,
    check: bool,
    no_warnings: bool,
    entrypoint: Option<String>,
    script_args: Vec<String>,
}

#[derive(Clone, Copy)]
enum NodeCliOptionSource {
    Environment,
    CommandLine,
}

impl NodeCliInvocation {
    fn parse(args: &[String], node_options: Option<&str>) -> Result<Self, String> {
        let mut invocation = Self {
            environment_requires: Vec::new(),
            environment_imports: Vec::new(),
            cli_requires: Vec::new(),
            cli_imports: Vec::new(),
            experimental_loaders: Vec::new(),
            exec_argv: Vec::new(),
            eval_source: None,
            input_type: None,
            check: false,
            no_warnings: false,
            entrypoint: None,
            script_args: Vec::new(),
        };

        if let Some(node_options) = node_options.filter(|value| !value.trim().is_empty()) {
            let environment_args = shlex::split(node_options)
                .ok_or_else(|| "invalid quoted NODE_OPTIONS".to_owned())?;
            invocation.parse_tokens(&environment_args, NodeCliOptionSource::Environment)?;
        }
        invocation.parse_tokens(args, NodeCliOptionSource::CommandLine)?;

        if invocation.eval_source.is_some() && invocation.check {
            return Err("--eval and --check cannot be used together".to_owned());
        }
        if invocation.eval_source.is_some() {
            if let Some(entrypoint) = invocation.entrypoint.take() {
                invocation.script_args.insert(0, entrypoint);
            }
        }

        Ok(invocation)
    }

    fn parse_tokens(&mut self, args: &[String], source: NodeCliOptionSource) -> Result<(), String> {
        let mut index = 0;
        let mut positional_only = false;
        while let Some(arg) = args.get(index) {
            let token_start = index;
            if matches!(source, NodeCliOptionSource::CommandLine) {
                if self.entrypoint.is_some() {
                    self.script_args.push(arg.clone());
                    index += 1;
                    continue;
                }
                if positional_only {
                    self.entrypoint = Some(arg.clone());
                    index += 1;
                    continue;
                }
            }

            if arg == "--" {
                if matches!(source, NodeCliOptionSource::Environment) {
                    return Err("NODE_OPTIONS cannot contain positional arguments".to_owned());
                }
                positional_only = true;
                index += 1;
                continue;
            }

            let (flag, value) = match arg.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_owned())),
                None => (arg.as_str(), None),
            };
            match flag {
                "--require" | "-r" => {
                    let value = Self::option_value(args, &mut index, flag, value)?;
                    self.add_require(source, value);
                }
                "--import" => {
                    let value = Self::option_value(args, &mut index, flag, value)?;
                    self.add_import(source, value);
                }
                "--experimental-loader" | "--loader" => {
                    let value = Self::option_value(args, &mut index, flag, value)?;
                    self.experimental_loaders.push(value);
                }
                "--eval" | "-e" => {
                    Self::reject_node_options_flag(source, flag)?;
                    let value = Self::option_value(args, &mut index, flag, value)?;
                    if self.eval_source.replace(value).is_some() {
                        return Err("multiple --eval options are unsupported".to_owned());
                    }
                }
                "--input-type" => {
                    let value = Self::option_value(args, &mut index, flag, value)?;
                    self.input_type = Some(value);
                }
                "--check" | "-c" => {
                    Self::reject_node_options_flag(source, flag)?;
                    Self::require_no_value(flag, value)?;
                    self.check = true;
                }
                "--no-warnings" => {
                    Self::require_no_value(flag, value)?;
                    self.no_warnings = true;
                }
                _ if flag.starts_with('-') => {
                    return Err(format!("unsupported Node CLI flag: {flag}"));
                }
                _ => {
                    if matches!(source, NodeCliOptionSource::Environment) {
                        return Err(format!(
                            "NODE_OPTIONS cannot contain positional argument: {arg}"
                        ));
                    }
                    self.entrypoint = Some(arg.clone());
                }
            }
            if matches!(source, NodeCliOptionSource::CommandLine) && flag.starts_with('-') {
                self.exec_argv
                    .extend(args[token_start..=index].iter().cloned());
            }
            index += 1;
        }
        Ok(())
    }

    fn option_value(
        args: &[String],
        index: &mut usize,
        flag: &str,
        value: Option<String>,
    ) -> Result<String, String> {
        match value {
            Some(value) if !value.is_empty() => Ok(value),
            Some(_) => Err(format!("{flag} requires a value")),
            None => {
                *index += 1;
                let value = args
                    .get(*index)
                    .filter(|value| !value.starts_with('-'))
                    .cloned()
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                Ok(value)
            }
        }
    }

    fn reject_node_options_flag(source: NodeCliOptionSource, flag: &str) -> Result<(), String> {
        if matches!(source, NodeCliOptionSource::Environment) {
            return Err(format!("{flag} is not allowed in NODE_OPTIONS"));
        }
        Ok(())
    }

    fn require_no_value(flag: &str, value: Option<String>) -> Result<(), String> {
        if value.is_some() {
            return Err(format!("{flag} does not accept a value"));
        }
        Ok(())
    }

    fn add_require(&mut self, source: NodeCliOptionSource, value: String) {
        match source {
            NodeCliOptionSource::Environment => self.environment_requires.push(value),
            NodeCliOptionSource::CommandLine => self.cli_requires.push(value),
        }
    }

    fn add_import(&mut self, source: NodeCliOptionSource, value: String) {
        match source {
            NodeCliOptionSource::Environment => self.environment_imports.push(value),
            NodeCliOptionSource::CommandLine => self.cli_imports.push(value),
        }
    }
}
#[derive(Deserialize)]
struct Inventory {
    source: InventorySource,
    tests: Vec<InventoryTest>,
}
#[derive(Deserialize)]
struct InventorySource {
    commit: String,
    node_version: String,
}
#[derive(Deserialize)]
struct InventoryTest {
    path: String,
    family: String,
    profile: String,
}
#[derive(Deserialize)]
struct Report {
    skipped: Option<String>,
    failures: Vec<String>,
}
enum Outcome {
    Pass,
    Skip(String),
    Assertion(String),
    Runtime(String),
    Timeout,
    Missing,
    Invalid(String),
}
fn test_name(path: &str) -> &str {
    Path::new(path).file_name().unwrap().to_str().unwrap()
}
fn is_esm(path: &str) -> bool {
    path.ends_with(".mjs")
}
fn rewrite_esm_harness_imports(body: &str) -> String {
    body.replace("'../common/index.mjs'", "'node-test:common'")
        .replace("\"../common/index.mjs\"", "\"node-test:common\"")
}
fn strip_shebang(body: &str) -> &str {
    body.strip_prefix("#!")
        .and_then(|rest| rest.split_once('\n').map(|(_, body)| body))
        .unwrap_or(body)
}
fn test_flags(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.trim_start().strip_prefix("// Flags:"))
        .flat_map(|flags| {
            shlex::split(flags)
                .unwrap_or_else(|| flags.split_whitespace().map(str::to_owned).collect())
        })
        .collect()
}
fn rewrite_dynamic_imports(body: &str) -> Option<String> {
    use swc_core::{
        common::{FileName, SourceMap, sync::Lrc},
        ecma::{
            ast::{Callee, Expr, Ident},
            codegen::{Emitter, text_writer::JsWriter},
            parser::{EsSyntax, Parser, StringInput, Syntax, lexer::Lexer},
            visit::{VisitMut, VisitMutWith},
        },
    };

    struct DynamicImportRewriter;

    impl VisitMut for DynamicImportRewriter {
        fn visit_mut_call_expr(&mut self, call: &mut swc_core::ecma::ast::CallExpr) {
            call.visit_mut_children_with(self);
            if matches!(call.callee, Callee::Import(_)) {
                call.callee = Callee::Expr(Box::new(Expr::Ident(Ident::new_no_ctxt(
                    "__nodeCompatImportWithLoaders".into(),
                    call.span,
                ))));
            }
        }
    }

    let source_map: Lrc<SourceMap> = Default::default();
    let source_file = source_map.new_source_file(
        FileName::Custom("node-compat-test.js".to_owned()).into(),
        body.to_owned(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax::default()),
        Default::default(),
        StringInput::from(&*source_file),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let mut script = parser.parse_script().ok()?;
    if !parser.take_errors().is_empty() {
        return None;
    }
    script.visit_mut_with(&mut DynamicImportRewriter);
    let mut output = Vec::new();
    Emitter {
        cfg: Default::default(),
        cm: source_map,
        comments: None,
        wr: JsWriter::new(Default::default(), "\n", &mut output, None),
    }
    .emit_script(&script)
    .ok()?;
    let output = String::from_utf8(output).ok()?;
    Some(format!(
        "const __nodeCompatImportWithLoaders = globalThis.__NODE_COMPAT_IMPORT_WITH_LOADERS__;\n{output}"
    ))
}

fn test_loader_specifier(loader: &str) -> Option<String> {
    let loader = loader.strip_prefix("./").unwrap_or(loader);
    if !loader.starts_with("test/") {
        return None;
    }
    ModuleSpecifier::from_file_path(Path::new("/").join(loader))
        .ok()
        .map(|specifier| specifier.to_string())
}

fn loader_resolve_prelude(path: &str, body: &str) -> Option<(String, String)> {
    let invocation = NodeCliInvocation::parse(&test_flags(body), None).ok()?;
    if invocation.experimental_loaders.is_empty() {
        return None;
    }
    let loaders = invocation
        .experimental_loaders
        .iter()
        .map(|loader| test_loader_specifier(loader))
        .collect::<Option<Vec<_>>>()?;
    let body = rewrite_dynamic_imports(body)?;
    let parent_url = test_module_specifier(path).ok()?;
    let prelude = format!(
        r#"const __nodeCompatResolveLoaders = await Promise.all({loaders}.map((specifier) => import(specifier)));
const __nodeCompatResolveHooks = __nodeCompatResolveLoaders.map((loader) => loader.resolve).filter((hook) => typeof hook === 'function');
const __nodeCompatDefaultResolve = async (specifier, context, originalImportAttributes) => {{
  const relative = specifier.startsWith('.') || specifier.startsWith('/') || specifier.startsWith('file:') || specifier.startsWith('data:');
  return {{ url: relative ? new URL(specifier, context.parentURL).href : specifier, importAttributes: originalImportAttributes }};
}};
const __nodeCompatRunResolve = async (index, specifier, context, originalImportAttributes) => {{
  if (index < 0) return __nodeCompatDefaultResolve(specifier, context, originalImportAttributes);
  const hook = __nodeCompatResolveHooks[index];
  return await hook(specifier, context, (nextSpecifier = specifier, nextContext = context) => __nodeCompatRunResolve(index - 1, nextSpecifier, nextContext, originalImportAttributes));
}};
globalThis.__NODE_COMPAT_IMPORT_WITH_LOADERS__ = async (specifier, options) => {{
  const originalImportAttributes = {{ ...(options?.with ?? {{}}) }};
  const context = {{ conditions: ['node', 'import'], importAttributes: {{ ...originalImportAttributes }}, parentURL: {parent_url} }};
  const resolved = await __nodeCompatRunResolve(__nodeCompatResolveHooks.length - 1, String(specifier), context, originalImportAttributes);
  const importAttributes = resolved?.importAttributes ?? context.importAttributes;
  const importOptions = Object.keys(importAttributes).length === 0 ? undefined : {{ with: importAttributes }};
  return import(resolved?.url ?? String(specifier), importOptions);
}};
"#,
        loaders = serde_json::to_string(&loaders).unwrap(),
        parent_url = serde_json::to_string(&parent_url).unwrap(),
    );
    Some((prelude, body))
}

fn assemble(path: &str, body: &str) -> String {
    let body = strip_shebang(body);
    if is_esm(path) {
        let body = rewrite_esm_harness_imports(body);
        let specifier = test_module_specifier(path).ok();
        let body = specifier
            .as_deref()
            .and_then(|specifier| rewrite_import_meta_resolve(specifier, &body))
            .unwrap_or(body);
        return format!(
            "import 'node-test:prelude';\n{}\nglobalThis.__NODE_TEST_SCHEDULE_REPORT__({:?});",
            body, SENTINEL,
        );
    }
    let (loader_prelude, body) =
        loader_resolve_prelude(path, body).unwrap_or_else(|| (String::new(), body.to_owned()));
    format!(
        "globalThis.__NODE_TEST_PATH__={};globalThis.__NODE_TEST_NAME__={};\n{}\n{}\ntry{{globalThis.__NODE_TEST_RUN_CJS__({});}}catch(e){{if(!(e&&e.__nodeTestSkip))throw e;}}\nglobalThis.__NODE_TEST_SCHEDULE_REPORT__({:?});",
        serde_json::to_string(path).unwrap(),
        serde_json::to_string(test_name(path)).unwrap(),
        PRELUDE,
        loader_prelude,
        serde_json::to_string(&body).unwrap(),
        SENTINEL
    )
}

const COMMON_ESM: &str = r#"import 'node-test:prelude';
const common = globalThis.__NODE_TEST_COMMON__;
export const mustCall = common.mustCall.bind(common);
export const mustCallAtLeast = common.mustCallAtLeast.bind(common);
export const mustSucceed = common.mustSucceed.bind(common);
export const mustNotCall = common.mustNotCall.bind(common);
export const spawnPromisified = common.spawnPromisified.bind(common);
export const skip = common.skip.bind(common);
export const platformTimeout = common.platformTimeout.bind(common);
export const fixturesDir = '/test/fixtures';
export default common;
"#;
fn package_uses_modules(directory: &Path, inherited: bool) -> bool {
    let package = directory.join("package.json");
    let Ok(source) = fs::read_to_string(package) else {
        return inherited;
    };
    serde_json::from_str::<serde_json::Value>(&source)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(|kind| kind.as_str())
                .map(str::to_owned)
        })
        .is_some_and(|kind| kind == "module")
}
fn source_uses_esm_syntax(specifier: &str, source: &str) -> bool {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    deno_ast::parse_program(deno_ast::ParseParams {
        specifier: match deno_ast::ModuleSpecifier::parse(specifier) {
            Ok(specifier) => specifier,
            Err(_) => return false,
        },
        text: source.into(),
        media_type: deno_ast::MediaType::JavaScript,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .is_ok_and(|parsed| !parsed.compute_is_script())
}
fn rewrite_import_meta_resolve(specifier: &str, source: &str) -> Option<String> {
    use swc_core::{
        common::{FileName, SourceMap, sync::Lrc},
        ecma::{
            ast::{Callee, Expr, Ident, MemberProp, MetaPropKind},
            codegen::{Emitter, text_writer::JsWriter},
            parser::{EsSyntax, Parser, StringInput, Syntax, lexer::Lexer},
            visit::{VisitMut, VisitMutWith},
        },
    };

    #[derive(Default)]
    struct ImportMetaResolveRewriter {
        rewritten: bool,
    }

    impl VisitMut for ImportMetaResolveRewriter {
        fn visit_mut_call_expr(&mut self, call: &mut swc_core::ecma::ast::CallExpr) {
            call.visit_mut_children_with(self);
            let Callee::Expr(callee) = &call.callee else {
                return;
            };
            let Expr::Member(member) = &**callee else {
                return;
            };
            let Expr::MetaProp(meta) = &*member.obj else {
                return;
            };
            let MemberProp::Ident(property) = &member.prop else {
                return;
            };
            if meta.kind == MetaPropKind::ImportMeta && property.sym == *"resolve" {
                self.rewritten = true;
                call.callee = Callee::Expr(Box::new(Expr::Ident(Ident::new_no_ctxt(
                    "__nodeCompatImportMetaResolve".into(),
                    call.span,
                ))));
            }
        }
    }

    let source_map: Lrc<SourceMap> = Default::default();
    let source_file = source_map.new_source_file(
        FileName::Custom(specifier.to_owned()).into(),
        source.to_owned(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax::default()),
        Default::default(),
        StringInput::from(&*source_file),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let mut module = parser.parse_module().ok()?;
    if !parser.take_errors().is_empty() {
        return None;
    }
    let mut rewriter = ImportMetaResolveRewriter::default();
    module.visit_mut_with(&mut rewriter);
    if !rewriter.rewritten {
        return Some(source.to_owned());
    }
    let mut output = Vec::new();
    Emitter {
        cfg: Default::default(),
        cm: source_map.clone(),
        comments: None,
        wr: JsWriter::new(source_map, "\n", &mut output, None),
    }
    .emit_module(&module)
    .ok()?;
    let output = String::from_utf8(output).ok()?;
    Some(format!(
        "const __nodeCompatImportMetaResolve = (specifier, parentURL = import.meta.url) => globalThis.__NODE_COMPAT_IMPORT_META_RESOLVE__(specifier, parentURL);\n{output}"
    ))
}

fn commonjs_analysis(source: &str) -> deno_ast::ModuleExportsAndReExports {
    if !source.contains("exports") && !source.contains("require") {
        return Default::default();
    }
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let Ok(parsed) = deno_ast::parse_script(deno_ast::ParseParams {
        specifier: deno_ast::ModuleSpecifier::parse("file:///commonjs.js").unwrap(),
        text: source.into(),
        media_type: deno_ast::MediaType::Cjs,
        capture_tokens: true,
        scope_analysis: false,
        maybe_syntax: None,
    }) else {
        return Default::default();
    };
    parsed.analyze_cjs()
}
fn node_commonjs_analysis(source: &str) -> deno_ast::ModuleExportsAndReExports {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let Ok(analysis) = merve::parse_commonjs(source) else {
        return Default::default();
    };
    deno_ast::ModuleExportsAndReExports {
        exports: analysis
            .exports()
            .map(|export| export.name.to_owned())
            .collect(),
        reexports: analysis
            .reexports()
            .map(|reexport| reexport.name.to_owned())
            .collect(),
    }
}
#[cfg(test)]
fn commonjs_export_names(source: &str) -> Vec<String> {
    commonjs_analysis(source).exports
}
fn resolve_commonjs_candidate(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return Some(candidate.to_owned());
    }
    if candidate.extension().is_none() {
        for extension in ["js", "cjs", "json"] {
            let path = candidate.with_extension(extension);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    if !candidate.is_dir() {
        return None;
    }
    if let Ok(source) = fs::read_to_string(candidate.join("package.json"))
        && let Ok(package) = serde_json::from_str::<serde_json::Value>(&source)
        && let Some(main) = package.get("main").and_then(|value| value.as_str())
        && let Some(path) = resolve_commonjs_candidate(&candidate.join(main))
    {
        return Some(path);
    }
    ["index.js", "index.cjs", "index.json"]
        .into_iter()
        .map(|name| candidate.join(name))
        .find(|path| path.is_file())
}
fn resolve_commonjs_reexport(root: &Path, importer: &Path, request: &str) -> Option<PathBuf> {
    let resolved = if request.starts_with('/') {
        resolve_commonjs_candidate(&root.join(request.trim_start_matches('/')))
    } else if request.starts_with('.') {
        resolve_commonjs_candidate(&importer.parent()?.join(request))
    } else {
        let mut directory = importer.parent()?;
        loop {
            if let Some(path) =
                resolve_commonjs_candidate(&directory.join("node_modules").join(request))
            {
                break Some(path);
            }
            if directory == root {
                break None;
            }
            directory = directory.parent()?;
            if !directory.starts_with(root) {
                break None;
            }
        }
    }?;
    resolved.starts_with(root).then_some(resolved)
}
fn commonjs_export_names_for_path(
    root: &Path,
    path: &Path,
    source: &str,
    visited: &mut HashSet<PathBuf>,
) -> Vec<String> {
    let path = path.to_owned();
    if !visited.insert(path.clone()) {
        return Vec::new();
    }
    let analysis = commonjs_analysis(source);
    let mut exports = analysis.exports;
    for request in analysis.reexports {
        let Some(reexport_path) = resolve_commonjs_reexport(root, &path, &request) else {
            continue;
        };
        if reexport_path.extension().and_then(|value| value.to_str()) == Some("json") {
            continue;
        }
        let Ok(reexport_source) = fs::read_to_string(&reexport_path) else {
            continue;
        };
        exports.extend(commonjs_export_names_for_path(
            root,
            &reexport_path,
            strip_shebang(&reexport_source),
            visited,
        ));
    }
    visited.remove(&path);
    exports.sort_unstable();
    exports.dedup();
    exports
}

fn node_commonjs_export_names_for_path(
    root: &Path,
    path: &Path,
    source: &str,
    visited: &mut HashSet<PathBuf>,
) -> Vec<String> {
    let path = path.to_owned();
    if !visited.insert(path.clone()) {
        return Vec::new();
    }
    let analysis = node_commonjs_analysis(source);
    let mut exports = analysis.exports;
    for request in analysis.reexports {
        let Some(reexport_path) = resolve_commonjs_reexport(root, &path, &request) else {
            continue;
        };
        if reexport_path.extension().and_then(|value| value.to_str()) == Some("json") {
            continue;
        }
        let Ok(reexport_source) = fs::read_to_string(&reexport_path) else {
            continue;
        };
        exports.extend(node_commonjs_export_names_for_path(
            root,
            &reexport_path,
            strip_shebang(&reexport_source),
            visited,
        ));
    }
    visited.remove(&path);
    exports.sort_unstable();
    exports.dedup();
    exports
}

fn module_export_name(name: &swc_core::ecma::ast::ModuleExportName) -> String {
    match name {
        swc_core::ecma::ast::ModuleExportName::Ident(identifier) => identifier.sym.to_string(),
        swc_core::ecma::ast::ModuleExportName::Str(string) => {
            string.value.to_string_lossy().into_owned()
        }
    }
}

fn named_commonjs_import_error(root: &Path, importer: &Path, source: &str) -> Option<String> {
    use swc_core::{
        common::{FileName, SourceMap, Spanned, sync::Lrc},
        ecma::{
            ast::{ImportSpecifier, ModuleDecl, ModuleItem},
            parser::{EsSyntax, Parser, StringInput, Syntax, lexer::Lexer},
        },
    };

    let source_map: Lrc<SourceMap> = Default::default();
    let source_file = source_map.new_source_file(
        FileName::Custom(importer.display().to_string()).into(),
        source.to_owned(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax::default()),
        Default::default(),
        StringInput::from(&*source_file),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser.parse_module().ok()?;
    if !parser.take_errors().is_empty() {
        return None;
    }

    for item in module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let request = import.src.value.to_string_lossy().into_owned();
        let node_name = request.strip_prefix("node:").unwrap_or(&request);
        if server::engine::node_compat::resolve_submodule(node_name).is_some() {
            continue;
        }
        let Some(target) = resolve_commonjs_reexport(root, importer, &request) else {
            continue;
        };
        let extension = target.extension().and_then(|value| value.to_str());
        let Some(target_directory) = target.parent() else {
            continue;
        };
        let target_uses_modules = package_uses_modules(
            target_directory,
            inherited_package_modules(root, target_directory),
        );
        let is_commonjs =
            extension == Some("cjs") || (extension == Some("js") && !target_uses_modules);
        if !is_commonjs {
            continue;
        }
        let Ok(target_source) = fs::read_to_string(&target) else {
            continue;
        };
        let exports = node_commonjs_export_names_for_path(
            root,
            &target,
            strip_shebang(&target_source),
            &mut HashSet::new(),
        );
        let named = import
            .specifiers
            .iter()
            .filter_map(|specifier| match specifier {
                ImportSpecifier::Named(named) if !named.is_type_only => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(module_export_name)
                        .unwrap_or_else(|| named.local.sym.to_string());
                    Some((imported, named.local.sym.to_string()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let Some((missing, _)) = named.iter().find(|(imported, _)| {
            imported != "default" && imported != "module.exports" && !exports.contains(imported)
        }) else {
            continue;
        };
        let span = import.span();
        let start = (span.lo.0 - source_file.start_pos.0) as usize;
        let end = (span.hi.0 - source_file.start_pos.0) as usize;
        let one_line = source
            .get(start..end)
            .is_some_and(|statement| !statement.contains(['\n', '\r']));
        let destructure = one_line.then(|| {
            named
                .iter()
                .map(|(imported, local)| {
                    if imported == local {
                        imported.clone()
                    } else {
                        format!("{imported}: {local}")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        });
        let mut message = format!(
            "Named export '{missing}' not found. The requested module '{request}' is a CommonJS module, which may not support all module.exports as named exports.\nCommonJS modules can always be imported via the default export, for example using:\n\nimport pkg from '{request}';\n"
        );
        if let Some(destructure) = destructure {
            message.push_str(&format!("const {{ {destructure} }} = pkg;\n"));
        }
        return Some(format!(
            "throw new SyntaxError({});\n",
            serde_json::to_string(&message).unwrap()
        ));
    }
    None
}

fn add_named_commonjs_import_errors(
    root: &Path,
    virtual_paths: bool,
    modules: &mut HashMap<String, String>,
) {
    let importer_errors = modules
        .iter()
        .filter_map(|(specifier, source)| {
            let module_specifier = ModuleSpecifier::parse(specifier).ok()?;
            if module_specifier.scheme() != "file" {
                return None;
            }
            let importer = if virtual_paths {
                root.join(module_specifier.path().trim_start_matches('/'))
            } else {
                module_specifier.to_file_path().ok()?
            };
            if !importer.starts_with(root) || !importer.is_file() {
                return None;
            }
            named_commonjs_import_error(root, &importer, source)
                .map(|error| (specifier.clone(), error))
        })
        .collect::<Vec<_>>();
    for (specifier, error) in importer_errors {
        if let Some(source) = modules.get_mut(&specifier) {
            source.insert_str(0, &error);
        }
    }
}

fn transpile_esm_to_commonjs(specifier: &str, source: &str) -> Option<String> {
    use swc_core::{
        common::{
            FileName, GLOBALS, Globals, Mark, SourceMap, comments::SingleThreadedComments,
            sync::Lrc,
        },
        ecma::{
            atoms::Atom,
            codegen::{Emitter, text_writer::JsWriter},
            parser::{EsSyntax, Parser, StringInput, Syntax, lexer::Lexer},
            transforms::{
                base::{
                    fixer::fixer,
                    helpers::{HELPERS, Helpers, inject_helpers},
                    hygiene::hygiene,
                    resolver,
                },
                module::{
                    common_js,
                    path::{ImportResolver, Resolver as ModuleResolver},
                },
            },
            visit::swc_ecma_ast::Pass,
        },
    };

    struct EsmImportResolver;

    impl ImportResolver for EsmImportResolver {
        fn resolve_import(
            &self,
            _base: &FileName,
            module_specifier: &str,
        ) -> Result<Atom, anyhow::Error> {
            Ok(format!("mcp-v8:esm-import:{module_specifier}").into())
        }
    }

    let source_map: Lrc<SourceMap> = Default::default();
    let source_file = source_map.new_source_file(
        FileName::Custom(specifier.to_owned()).into(),
        source.to_owned(),
    );
    let comments = SingleThreadedComments::default();
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax::default()),
        Default::default(),
        StringInput::from(&*source_file),
        Some(&comments),
    );
    let mut parser = Parser::new_from(lexer);
    let mut program =
        swc_core::ecma::visit::swc_ecma_ast::Program::Module(parser.parse_module().ok()?);
    if !parser.take_errors().is_empty() {
        return None;
    }

    let output = GLOBALS.set(&Globals::default(), || {
        HELPERS.set(&Helpers::new(false), || {
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            resolver(unresolved_mark, top_level_mark, false).process(&mut program);
            common_js(
                ModuleResolver::Real {
                    base: FileName::Custom(specifier.to_owned()),
                    resolver: std::sync::Arc::new(EsmImportResolver),
                },
                unresolved_mark,
                Default::default(),
                Default::default(),
            )
            .process(&mut program);
            inject_helpers(top_level_mark).process(&mut program);
            hygiene().process(&mut program);
            fixer(Some(&comments)).process(&mut program);

            let mut output = Vec::new();
            Emitter {
                cfg: Default::default(),
                cm: source_map.clone(),
                comments: Some(&comments),
                wr: JsWriter::new(source_map, "\n", &mut output, None),
            }
            .emit_program(&program)
            .ok()?;
            String::from_utf8(output).ok()
        })
    })?;
    let original = url::form_urlencoded::byte_serialize(source.as_bytes()).collect::<String>();
    Some(format!("/*mcp-v8-original-esm:{original}*/\n{output}"))
}

fn wrap_commonjs_with_names(source: &str, export_names: Vec<String>) -> String {
    let mut wrapped = format!(
        r#"import {{ createRequire as __createRequire }} from 'node:module';
import {{ fileURLToPath as __fileURLToPath }} from 'node:url';
import {{ dirname as __dirnameOf }} from 'node:path';
const __cjsModule = {{ exports: {{}} }};
let __cjsExports = __cjsModule.exports;
const __cjsRequire = __createRequire(import.meta.url);
const __cjsFilename = __fileURLToPath(import.meta.url);
const __cjsDirname = __dirnameOf(__cjsFilename);
(function (exports, require, module, __filename, __dirname) {{
{source}
}}).call(__cjsModule.exports, __cjsExports, __cjsRequire, __cjsModule, __cjsFilename, __cjsDirname);
const __cjsDefault = __cjsModule.exports;
export default __cjsDefault;
export {{ __cjsDefault as 'module.exports' }};
"#
    );
    for (index, name) in export_names
        .into_iter()
        .filter(|name| name != "default" && name != "module.exports")
        .enumerate()
    {
        let quoted = serde_json::to_string(&name).unwrap();
        wrapped.push_str(&format!(
            "const __cjsNamedExport{index} = __cjsModule.exports[{quoted}];\n\
             export {{ __cjsNamedExport{index} as {quoted} }};\n"
        ));
    }
    wrapped
}
#[cfg(test)]
fn wrap_commonjs(source: &str) -> String {
    wrap_commonjs_with_names(source, commonjs_export_names(source))
}
struct CorpusModules {
    esm: Arc<HashMap<String, String>>,
    commonjs: Arc<HashMap<String, String>>,
    files: Arc<HashSet<String>>,
}
fn add_corpus_modules(
    root: &Path,
    directory: &Path,
    inherited_modules: bool,
    modules: &mut HashMap<String, String>,
    commonjs_modules: &mut HashMap<String, String>,
    files: &mut HashSet<String>,
) -> Result<(), String> {
    let directory_uses_modules = package_uses_modules(directory, inherited_modules);
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            add_corpus_modules(
                root,
                &path,
                directory_uses_modules,
                modules,
                commonjs_modules,
                files,
            )?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|error| error.to_string())?;
        let virtual_path = Path::new("/").join(relative);
        let specifier = ModuleSpecifier::from_file_path(&virtual_path)
            .map_err(|_| format!("invalid corpus file path: {}", virtual_path.display()))?;
        files.insert(specifier.to_string());
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("js" | "mjs" | "cjs" | "json")
        ) {
            continue;
        }
        let source = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let source = strip_shebang(&source);
        let extension = path.extension().and_then(|value| value.to_str());
        let detected_esm = extension == Some("mjs")
            || (extension == Some("js")
                && (directory_uses_modules || source_uses_esm_syntax(specifier.as_str(), source)));
        let is_commonjs = extension == Some("cjs") || (extension == Some("js") && !detected_esm);

        if extension == Some("json") || is_commonjs {
            commonjs_modules.insert(specifier.to_string(), source.to_string());
        } else if detected_esm
            && let Some(commonjs) = transpile_esm_to_commonjs(specifier.as_str(), source)
        {
            commonjs_modules.insert(specifier.to_string(), commonjs);
        }

        let module_source = if is_commonjs {
            let export_names =
                commonjs_export_names_for_path(root, &path, source, &mut HashSet::new());
            wrap_commonjs_with_names(source, export_names)
        } else if detected_esm {
            rewrite_import_meta_resolve(specifier.as_str(), source)
                .unwrap_or_else(|| source.to_string())
        } else {
            source.to_string()
        };
        modules.insert(specifier.to_string(), module_source);
    }
    Ok(())
}
fn corpus_modules(corpus: &Path) -> Result<CorpusModules, String> {
    let mut modules = HashMap::new();
    let mut commonjs_modules = HashMap::new();
    let mut files = HashSet::new();
    add_corpus_modules(
        corpus,
        &corpus.join("test"),
        false,
        &mut modules,
        &mut commonjs_modules,
        &mut files,
    )?;
    add_named_commonjs_import_errors(corpus, true, &mut modules);
    modules.insert("node-test:prelude".into(), PRELUDE.into());
    modules.insert("node-test:common".into(), COMMON_ESM.into());
    modules.insert("file:///test/common/index.mjs".into(), COMMON_ESM.into());
    Ok(CorpusModules {
        esm: Arc::new(modules),
        commonjs: Arc::new(commonjs_modules),
        files: Arc::new(files),
    })
}
fn inherited_package_modules(test_root: &Path, directory: &Path) -> bool {
    let mut ancestors = directory
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|ancestor| ancestor.starts_with(test_root))
        .collect::<Vec<_>>();
    ancestors.reverse();
    ancestors.into_iter().fold(false, |inherited, ancestor| {
        package_uses_modules(ancestor, inherited)
    })
}
fn add_host_modules(
    root: &Path,
    directory: &Path,
    inherited_modules: bool,
    modules: &mut HashMap<String, String>,
    commonjs_modules: &mut HashMap<String, String>,
    files: &mut HashSet<String>,
) -> Result<(), String> {
    let directory_uses_modules = package_uses_modules(directory, inherited_modules);
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            add_host_modules(
                root,
                &path,
                directory_uses_modules,
                modules,
                commonjs_modules,
                files,
            )?;
            continue;
        }
        let specifier = ModuleSpecifier::from_file_path(&path)
            .map_err(|_| format!("invalid host module path: {}", path.display()))?;
        files.insert(specifier.to_string());
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("js" | "mjs" | "cjs" | "json")
        ) {
            continue;
        }
        let source = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let source = strip_shebang(&source);
        let extension = path.extension().and_then(|value| value.to_str());
        let detected_esm = extension == Some("mjs")
            || (extension == Some("js")
                && (directory_uses_modules || source_uses_esm_syntax(specifier.as_str(), source)));
        let is_commonjs = extension == Some("cjs") || (extension == Some("js") && !detected_esm);

        if extension == Some("json") || is_commonjs {
            commonjs_modules.insert(specifier.to_string(), source.to_string());
        } else if detected_esm
            && let Some(commonjs) = transpile_esm_to_commonjs(specifier.as_str(), source)
        {
            commonjs_modules.insert(specifier.to_string(), commonjs);
        }
        let module_source = if is_commonjs {
            let export_names =
                commonjs_export_names_for_path(root, &path, source, &mut HashSet::new());
            wrap_commonjs_with_names(source, export_names)
        } else if detected_esm {
            rewrite_import_meta_resolve(specifier.as_str(), source)
                .unwrap_or_else(|| source.to_string())
        } else {
            source.to_string()
        };
        modules.insert(specifier.to_string(), module_source);
    }
    Ok(())
}

fn corpus_modules_for_cli(
    corpus: &Path,
    invocation: &NodeCliInvocation,
) -> Result<CorpusModules, String> {
    let test_root = corpus.join("test");
    let mut directories = HashSet::new();
    let mut needs_common = false;
    let module_paths = invocation
        .environment_requires
        .iter()
        .chain(&invocation.cli_requires)
        .chain(&invocation.environment_imports)
        .chain(&invocation.cli_imports)
        .chain(&invocation.experimental_loaders)
        .chain(invocation.entrypoint.iter());
    for value in module_paths {
        let Ok(virtual_path) = virtualize_corpus_path(value, corpus) else {
            continue;
        };
        let path = virtual_corpus_file(&virtual_path, corpus)?;
        let Some(directory) = path.parent() else {
            continue;
        };
        directories.insert(directory.to_owned());
        for ancestor in directory
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(&test_root))
        {
            let node_modules = ancestor.join("node_modules");
            if node_modules.is_dir() {
                directories.insert(node_modules);
            }
        }
        if !path.starts_with(corpus.join("test/fixtures")) {
            needs_common = true;
        }
    }
    let common = corpus.join("test/common");
    if needs_common && common.is_dir() {
        directories.insert(common);
    }

    let mut modules = HashMap::new();
    let mut commonjs_modules = HashMap::new();
    let mut files = HashSet::new();
    for directory in directories {
        add_corpus_modules(
            corpus,
            &directory,
            inherited_package_modules(&test_root, &directory),
            &mut modules,
            &mut commonjs_modules,
            &mut files,
        )?;
    }
    add_named_commonjs_import_errors(corpus, true, &mut modules);
    if let Ok(file_root) = std::env::var("NODE_COMPAT_FILE_ROOT") {
        let file_root = fs::canonicalize(file_root).map_err(|error| error.to_string())?;
        let cwd = fs::canonicalize(std::env::current_dir().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        if !cwd.starts_with(&file_root) {
            return Err("Node CLI cwd is outside NODE_COMPAT_FILE_ROOT".to_owned());
        }
        add_host_modules(
            &file_root,
            &cwd,
            inherited_package_modules(&file_root, &cwd),
            &mut modules,
            &mut commonjs_modules,
            &mut files,
        )?;
        add_named_commonjs_import_errors(&file_root, false, &mut modules);
        let root = ModuleSpecifier::from_directory_path(&file_root)
            .map_err(|_| "invalid NODE_COMPAT_FILE_ROOT".to_owned())?;
        files.insert(format!("mcp-v8:file-root:{root}"));
    }
    modules.insert("node-test:prelude".into(), PRELUDE.into());
    modules.insert("node-test:common".into(), COMMON_ESM.into());
    modules.insert("file:///test/common/index.mjs".into(), COMMON_ESM.into());
    Ok(CorpusModules {
        esm: Arc::new(modules),
        commonjs: Arc::new(commonjs_modules),
        files: Arc::new(files),
    })
}
fn test_module_specifier(path: &str) -> Result<String, String> {
    let virtual_path = Path::new("/").join(path);
    ModuleSpecifier::from_file_path(&virtual_path)
        .map(|specifier| specifier.to_string())
        .map_err(|_| format!("invalid test module path: {}", virtual_path.display()))
}
#[derive(Debug)]
struct NodeCliOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    runtime_error: Option<String>,
}

fn is_virtual_test_path(path: &Path) -> bool {
    path == Path::new("/test") || path.starts_with("/test/")
}

fn virtualize_corpus_path(value: &str, corpus: &Path) -> Result<String, String> {
    let is_file_url = value.starts_with("file:");
    let path = if is_file_url {
        let specifier = ModuleSpecifier::parse(value)
            .map_err(|error| format!("invalid file URL {value}: {error}"))?;
        if specifier.scheme() != "file" {
            return Err(format!("unsupported preload URL: {value}"));
        }
        specifier
            .to_file_path()
            .map_err(|_| format!("invalid file URL path: {value}"))?
    } else {
        PathBuf::from(value)
    };

    if is_virtual_test_path(&path) {
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(format!("invalid virtual corpus path: {value}"));
        }
        return if is_file_url {
            ModuleSpecifier::from_file_path(&path)
                .map(|specifier| specifier.to_string())
                .map_err(|_| format!("invalid virtual corpus path: {value}"))
        } else {
            Ok(path.to_string_lossy().into_owned())
        };
    }

    let corpus = fs::canonicalize(corpus).map_err(|error| error.to_string())?;
    let path =
        fs::canonicalize(&path).map_err(|error| format!("invalid corpus path {value}: {error}"))?;
    let relative = path
        .strip_prefix(&corpus)
        .map_err(|_| format!("path is outside NODE_COMPAT_CORPUS: {value}"))?;
    if !relative.starts_with("test") {
        return Err(format!("path is outside corpus test directory: {value}"));
    }
    let virtual_path = Path::new("/").join(relative);
    if is_file_url {
        ModuleSpecifier::from_file_path(&virtual_path)
            .map(|specifier| specifier.to_string())
            .map_err(|_| format!("invalid corpus path: {value}"))
    } else {
        Ok(virtual_path.to_string_lossy().into_owned())
    }
}

fn cli_path_module_specifier(value: &str) -> Result<String, String> {
    if value.starts_with("file:") {
        return ModuleSpecifier::parse(value)
            .map(|specifier| specifier.to_string())
            .map_err(|error| format!("invalid module specifier {value}: {error}"));
    }
    ModuleSpecifier::from_file_path(value)
        .map(|specifier| specifier.to_string())
        .map_err(|_| format!("invalid virtual module path: {value}"))
}

fn virtualize_cli_module(value: &str, corpus: &Path) -> Result<String, String> {
    if value.starts_with("file:") || Path::new(value).is_absolute() {
        let path = virtualize_corpus_path(value, corpus)?;
        cli_path_module_specifier(&path)
    } else {
        Ok(value.to_owned())
    }
}

fn virtual_corpus_file(specifier: &str, corpus: &Path) -> Result<PathBuf, String> {
    let specifier = cli_path_module_specifier(specifier)?;
    let specifier = ModuleSpecifier::parse(&specifier)
        .map_err(|error| format!("invalid virtual corpus specifier: {error}"))?;
    let path = specifier
        .to_file_path()
        .map_err(|_| format!("invalid virtual corpus specifier: {specifier}"))?;
    if !is_virtual_test_path(&path) {
        return Err(format!(
            "path is outside corpus test directory: {specifier}"
        ));
    }
    let relative = path
        .strip_prefix("/test")
        .map_err(|_| format!("path is outside corpus test directory: {specifier}"))?;
    Ok(corpus.join("test").join(relative))
}

fn append_cli_modules(
    source: &mut String,
    modules: &[String],
    corpus: &Path,
) -> Result<(), String> {
    for module in modules {
        let module = virtualize_cli_module(module, corpus)?;
        source.push_str("await import(");
        source.push_str(&serde_json::to_string(&module).unwrap());
        source.push_str(");\n");
    }
    Ok(())
}

fn node_cli_exec_argv(invocation: &NodeCliInvocation) -> Vec<String> {
    invocation.exec_argv.clone()
}

fn node_cli_process_config(
    invocation: &NodeCliInvocation,
    corpus: &Path,
) -> Result<serde_json::Value, String> {
    let exec_path = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut argv = vec![exec_path.to_string_lossy().into_owned()];
    if let Some(entrypoint) = &invocation.entrypoint {
        argv.push(virtualize_corpus_path(entrypoint, corpus)?);
    }
    argv.extend(invocation.script_args.iter().cloned());
    let environment = std::env::vars().collect::<HashMap<_, _>>();
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    Ok(serde_json::json!({
        "argv": argv,
        "argv0": exec_path,
        "execArgv": node_cli_exec_argv(invocation),
        "execPath": exec_path,
        "env": environment,
        "cwd": cwd,
        "hostExit": true,
    }))
}

fn append_commonjs_eval(source: &mut String, eval: &str) {
    source.push_str(
        "const { createRequire: __nodeCompatCreateRequire } = await import('node:module');\n\
         const __nodeCompatFilename = '/test/[eval].js';\n\
         const __nodeCompatDirname = '/test';\n\
         const __nodeCompatModule = { exports: {} };\n\
         const __nodeCompatExports = __nodeCompatModule.exports;\n\
         const __nodeCompatRequire = __nodeCompatCreateRequire('file:///test/[eval].js');\n\
         (function (exports, require, module, __filename, __dirname) {\n",
    );
    source.push_str(eval);
    source.push_str(
        "\n}).call(__nodeCompatModule.exports, __nodeCompatExports, __nodeCompatRequire, \
         __nodeCompatModule, __nodeCompatFilename, __nodeCompatDirname);\n",
    );
}

fn package_check_mode(path: &Path, corpus: &Path) -> CompileMode {
    let mut directory = path.parent();
    let test_root = corpus.join("test");
    while let Some(current) = directory {
        if !current.starts_with(&test_root) {
            break;
        }
        if let Ok(source) = fs::read_to_string(current.join("package.json")) {
            if let Ok(package) = serde_json::from_str::<serde_json::Value>(&source) {
                return match package.get("type").and_then(|value| value.as_str()) {
                    Some("module") => CompileMode::EsModule,
                    Some("commonjs") => CompileMode::CommonJs,
                    _ => CompileMode::Ambiguous,
                };
            }
        }
        directory = current.parent();
    }
    CompileMode::Ambiguous
}

fn check_compile_mode(path: &Path, corpus: &Path) -> CompileMode {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("cjs") => CompileMode::CommonJs,
        Some("mjs") => CompileMode::EsModule,
        Some("js") => package_check_mode(path, corpus),
        _ => CompileMode::Ambiguous,
    }
}

fn node_cli_source(
    invocation: &NodeCliInvocation,
    corpus: &Path,
) -> Result<(String, Option<(String, String, CompileMode)>), String> {
    let process_config = node_cli_process_config(invocation, corpus)?;
    let mut source = format!(
        "globalThis.__mcpV8ProcessConfig={};\n\
         const {{ default: __nodeCompatProcess }} = await import('node:process');\n\
         globalThis.process = __nodeCompatProcess;\n\
         globalThis.global = globalThis;\n",
        serde_json::to_string(&process_config).unwrap(),
    );
    append_cli_modules(&mut source, &invocation.environment_requires, corpus)?;
    append_cli_modules(&mut source, &invocation.cli_requires, corpus)?;
    append_cli_modules(&mut source, &invocation.environment_imports, corpus)?;
    append_cli_modules(&mut source, &invocation.cli_imports, corpus)?;

    if let Some(eval) = &invocation.eval_source {
        match invocation.input_type.as_deref().unwrap_or("commonjs") {
            "commonjs" => append_commonjs_eval(&mut source, eval),
            "module" => {
                source.push_str(eval);
                source.push('\n');
            }
            input_type => return Err(format!("unsupported --input-type: {input_type}")),
        }
        return Ok((source, None));
    }
    if invocation.input_type.is_some() {
        return Err("--input-type requires --eval".to_owned());
    }

    let Some(entrypoint) = &invocation.entrypoint else {
        return Ok((source, None));
    };
    let entrypoint = virtualize_corpus_path(entrypoint, corpus)?;
    if invocation.check {
        let source_path = virtual_corpus_file(&entrypoint, corpus)?;
        let entry_source = fs::read_to_string(&source_path).map_err(|error| error.to_string())?;
        let mode = check_compile_mode(&source_path, corpus);
        return Ok((
            source,
            Some((entrypoint, strip_shebang(&entry_source).to_owned(), mode)),
        ));
    }

    let source_path = virtual_corpus_file(&entrypoint, corpus)?;
    let entry_source =
        strip_shebang(&fs::read_to_string(&source_path).map_err(|error| error.to_string())?)
            .to_owned();
    let entrypoint = cli_path_module_specifier(&entrypoint)?;
    if invocation.experimental_loaders.is_empty() {
        source.push_str("await import(");
        source.push_str(&serde_json::to_string(&entrypoint).unwrap());
        source.push_str(");\n");
        return Ok((source, None));
    }

    let loaders = invocation
        .experimental_loaders
        .iter()
        .map(|loader| {
            let loader = virtualize_corpus_path(loader, corpus)?;
            cli_path_module_specifier(&loader)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let format = match check_compile_mode(&source_path, corpus) {
        CompileMode::CommonJs => "commonjs",
        _ => "module",
    };
    if !invocation.no_warnings {
        source.push_str("console.error('ExperimentalWarning: `--experimental-loader` may be removed in the future');\n");
    }
    source.push_str(&format!(
        r#"const __nodeCompatLoaderModules = await Promise.all({loaders}.map((specifier) => import(specifier)));
const __nodeCompatLoadHooks = __nodeCompatLoaderModules.map((loader) => loader.load).filter((hook) => typeof hook === 'function');
const __nodeCompatEntryUrl = {entrypoint};
const __nodeCompatDefaultSource = {entry_source};
const __nodeCompatDefaultLoad = async (url, context) => ({{ format: {format}, source: __nodeCompatDefaultSource, url }});
const __nodeCompatRunLoad = async (index, url, context) => {{
  if (index < 0) return __nodeCompatDefaultLoad(url, context);
  const hook = __nodeCompatLoadHooks[index];
  return await hook(url, context, (nextUrl = url, nextContext = context) => __nodeCompatRunLoad(index - 1, nextUrl, nextContext));
}};
const __nodeCompatLoaded = await __nodeCompatRunLoad(__nodeCompatLoadHooks.length - 1, __nodeCompatEntryUrl, {{ format: {format}, importAttributes: {{}} }});
if (__nodeCompatLoaded == null || typeof __nodeCompatLoaded !== 'object') throw new TypeError('loader load hook must return an object');
if (__nodeCompatLoaded.source === __nodeCompatDefaultSource && __nodeCompatLoaded.format === {format}) {{
  await import(__nodeCompatEntryUrl);
}} else {{
  await import('data:text/javascript;charset=utf-8,' + encodeURIComponent(String(__nodeCompatLoaded.source ?? '')));
}}
"#,
        loaders = serde_json::to_string(&loaders).unwrap(),
        entrypoint = serde_json::to_string(&entrypoint).unwrap(),
        entry_source = serde_json::to_string(&entry_source).unwrap(),
        format = serde_json::to_string(format).unwrap(),
    ));
    Ok((source, None))
}

fn cjs_esm_load_note(
    invocation: &NodeCliInvocation,
    corpus: &Path,
    runtime_error: Option<&str>,
) -> Option<String> {
    if !runtime_error.is_some_and(|error| error.starts_with("SyntaxError:")) {
        return None;
    }
    let entrypoint = invocation.entrypoint.as_ref()?;
    let virtual_path = virtualize_corpus_path(entrypoint, corpus).ok()?;
    let source_path = virtual_corpus_file(&virtual_path, corpus).ok()?;
    if source_path.extension().and_then(|value| value.to_str()) != Some("cjs") {
        return None;
    }
    let source = fs::read_to_string(source_path).ok()?;
    let specifier = cli_path_module_specifier(&virtual_path).ok()?;
    source_uses_esm_syntax(&specifier, strip_shebang(&source)).then(|| {
        format!(
            "Warning: Failed to load the ES module: {virtual_path}. Make sure to set \"type\": \"module\" in the nearest package.json file or use the .mjs extension.\n"
        )
    })
}

fn run_node_compat_cli(
    args: &[String],
    node_options: Option<&str>,
    corpus: &Path,
) -> Result<NodeCliOutput, String> {
    let invocation = NodeCliInvocation::parse(args, node_options)?;
    let corpus = fs::canonicalize(corpus).map_err(|error| error.to_string())?;
    let modules = corpus_modules_for_cli(&corpus, &invocation)?;
    let (source, compile_module) = node_cli_source(&invocation, &corpus)?;

    INIT.call_once(server::engine::initialize_v8);
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    let database = sled::open(temporary.path()).map_err(|error| error.to_string())?;
    let tree = database
        .open_tree("console")
        .map_err(|error| error.to_string())?;
    let error_tree = database
        .open_tree("console-error")
        .map_err(|error| error.to_string())?;
    let policy = Arc::new(PolicyChain::new(vec![], EvalMode::All));
    let fetch = FetchConfig::new_with_chain(policy.clone());
    let subprocess = SubprocessConfig::new(policy);
    let process_exit_state = ProcessExitState::default();
    let module_loader = ModuleLoaderConfig {
        allow_external: true,
        policy_chain: None,
        virtual_modules: Some(modules.esm.clone()),
        virtual_commonjs_modules: Some(modules.commonjs.clone()),
        virtual_files: Some(modules.files.clone()),
    };
    let eval_main_specifier = invocation.eval_source.as_ref().map(|_| {
        let name = if invocation.input_type.as_deref() == Some("module") {
            "[eval1]"
        } else {
            "[eval]"
        };
        ModuleSpecifier::from_file_path(std::env::current_dir().unwrap().join(name))
            .unwrap()
            .to_string()
    });
    let mut config = ExecutionConfig::new(256 * 1024 * 1024)
        .console_tree(tree.clone())
        .console_error_tree(error_tree.clone())
        .process_exit_state(&process_exit_state)
        .fetch_config(&fetch)
        .maybe_subprocess_config(Some(&subprocess))
        .module_loader_config(&module_loader);
    if let Some(main_specifier) = eval_main_specifier.as_deref() {
        config = config.main_module_specifier(main_specifier);
    }
    if let Some((specifier, source, mode)) = compile_module.as_ref() {
        config = config.compile_module(specifier, source, *mode);
    }
    let (result, _) = server::engine::execute_stateless(&source, config);
    let mut stdout = Vec::new();
    for entry in tree.iter().flatten() {
        stdout.extend_from_slice(&entry.1);
    }
    let mut stderr = Vec::new();
    for entry in error_tree.iter().flatten() {
        stderr.extend_from_slice(&entry.1);
    }
    let runtime_error = if process_exit_state.exit_requested() {
        None
    } else {
        result.err()
    };
    let mut stderr = String::from_utf8_lossy(&stderr).into_owned();
    if let Some(note) = cjs_esm_load_note(&invocation, &corpus, runtime_error.as_deref()) {
        stderr.push_str(&note);
    }
    Ok(NodeCliOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr,
        exit_code: process_exit_state.exit_code(),
        runtime_error,
    })
}

fn node_compat_cli_main(args: &[String]) -> Result<(), String> {
    let corpus =
        fs::canonicalize(path_env("NODE_COMPAT_CORPUS")?).map_err(|error| error.to_string())?;
    let output = run_node_compat_cli(args, std::env::var("NODE_OPTIONS").ok().as_deref(), &corpus)?;
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    std::io::stdout()
        .flush()
        .map_err(|error| error.to_string())?;
    if let Some(error) = output.runtime_error {
        eprintln!("{error}");
        std::io::stderr()
            .flush()
            .map_err(|error| error.to_string())?;
        std::process::exit(1);
    }
    std::io::stderr()
        .flush()
        .map_err(|error| error.to_string())?;
    if output.exit_code != 0 {
        std::process::exit(output.exit_code);
    }
    Ok(())
}

fn test_tmpdir_modules(path: &Path) -> (String, String) {
    let path = serde_json::to_string(path.to_str().unwrap()).unwrap();
    let commonjs = format!(
        "const path = require('path');\n\
         const {{ pathToFileURL }} = require('url');\n\
         let tmpPath = {path};\n\
         const tmpdir = {{\n\
           refresh() {{}},\n\
           resolve: (...args) => path.resolve(tmpPath, ...args),\n\
           fileURL: (...args) => pathToFileURL(path.resolve(tmpPath, ...args)),\n\
           hasEnoughSpace: () => true,\n\
           get path() {{ return tmpPath; }},\n\
           set path(value) {{ tmpPath = path.resolve(String(value)); }},\n\
         }};\n\
         module.exports = tmpdir;\n"
    );
    let esm = format!(
        "import path from 'node:path';\n\
         import {{ pathToFileURL }} from 'node:url';\n\
         let tmpPath = {path};\n\
         export function refresh() {{}}\n\
         export const resolve = (...args) => path.resolve(tmpPath, ...args);\n\
         export const fileURL = (...args) => pathToFileURL(path.resolve(tmpPath, ...args));\n\
         export const hasEnoughSpace = () => true;\n\
         const tmpdir = {{ refresh, resolve, fileURL, hasEnoughSpace,\n\
           get path() {{ return tmpPath; }},\n\
           set path(value) {{ tmpPath = path.resolve(String(value)); }},\n\
         }};\n\
         export default tmpdir;\n"
    );
    (esm, commonjs)
}

fn isolated_fs_policy(root: &Path) -> Result<(tempfile::TempDir, Arc<PolicyChain>), String> {
    let policy_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let root = format!("{}/", root.to_string_lossy().trim_end_matches('/'));
    let source = format!(
        "package mcp.node_compat\n\ndefault allow = false\n\nallow if {{\n    startswith(input.path, {})\n}}\n",
        serde_json::to_string(&root).unwrap(),
    );
    let policy_path = policy_dir.path().join("fs.rego");
    fs::write(&policy_path, source).map_err(|error| error.to_string())?;
    let evaluator =
        LocalPolicyEvaluator::from_file(&policy_path, "data.mcp.node_compat.allow".to_owned())?;
    Ok((
        policy_dir,
        Arc::new(PolicyChain::new(
            vec![PolicyEvaluatorKind::Local(evaluator)],
            EvalMode::All,
        )),
    ))
}

fn run(
    path: &str,
    body: &str,
    timeout: Duration,
    modules: &CorpusModules,
    corpus: &Path,
    node_executable: &Path,
) -> Outcome {
    INIT.call_once(server::engine::initialize_v8);
    let tmp = std::env::temp_dir().join(format!(
        "mcp-node-full-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = match sled::open(&tmp) {
        Ok(x) => x,
        Err(e) => return Outcome::Runtime(e.to_string()),
    };
    let tree = db.open_tree("console").unwrap();
    let test_tmp = match tempfile::tempdir() {
        Ok(directory) => directory,
        Err(error) => return Outcome::Runtime(error.to_string()),
    };
    let policy = Arc::new(PolicyChain::new(vec![], EvalMode::All));
    let fetch = FetchConfig::new_with_chain(policy.clone());
    let (_fs_policy_dir, fs_policy) = match isolated_fs_policy(test_tmp.path()) {
        Ok(policy) => policy,
        Err(error) => return Outcome::Runtime(error),
    };
    let fs_config = FsConfig::new(fs_policy);
    let subprocess = SubprocessConfig::new(policy);
    let (tmpdir_esm, tmpdir_commonjs) = test_tmpdir_modules(test_tmp.path());
    let mut virtual_modules = (*modules.esm).clone();
    virtual_modules.insert("file:///test/common/tmpdir.js".to_owned(), tmpdir_esm);
    let mut virtual_commonjs_modules = (*modules.commonjs).clone();
    virtual_commonjs_modules.insert("file:///test/common/tmpdir.js".to_owned(), tmpdir_commonjs);
    let mut virtual_files = (*modules.files).clone();
    let file_root = match ModuleSpecifier::from_directory_path(test_tmp.path()) {
        Ok(specifier) => format!("mcp-v8:file-root:{specifier}"),
        Err(()) => return Outcome::Runtime("invalid module file root".to_owned()),
    };
    virtual_files.insert(file_root);
    let module_loader = ModuleLoaderConfig {
        allow_external: true,
        policy_chain: None,
        virtual_modules: Some(Arc::new(virtual_modules)),
        virtual_commonjs_modules: Some(Arc::new(virtual_commonjs_modules)),
        virtual_files: Some(Arc::new(virtual_files)),
    };
    let main_specifier = match test_module_specifier(path) {
        Ok(specifier) => specifier,
        Err(error) => return Outcome::Runtime(error),
    };
    let config = ExecutionConfig::new(256 * 1024 * 1024)
        .console_tree(tree.clone())
        .fetch_config(&fetch)
        .maybe_fs_config(Some(&fs_config))
        .maybe_subprocess_config(Some(&subprocess))
        .module_loader_config(&module_loader)
        .main_module_specifier(&main_specifier);
    let handle = config.isolate_handle.clone();
    let done = Arc::new(AtomicBool::new(false));
    let timed = Arc::new(AtomicBool::new(false));
    let wd = {
        let done = done.clone();
        let timed = timed.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            while !done.load(Ordering::SeqCst) {
                if start.elapsed() > timeout {
                    timed.store(true, Ordering::SeqCst);
                    if let Some(h) = handle.lock().unwrap().as_ref() {
                        h.terminate_execution();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(25))
            }
        })
    };
    let source = format!(
        "globalThis.__NODE_TEST_CORPUS_HOST__={};globalThis.__NODE_TEST_EXEC_PATH__={};globalThis.__NODE_TEST_FLAGS__={};globalThis.__NODE_TEST_TMPDIR__={};\n{}",
        serde_json::to_string(corpus.to_str().unwrap()).unwrap(),
        serde_json::to_string(node_executable.to_str().unwrap()).unwrap(),
        serde_json::to_string(&test_flags(body)).unwrap(),
        serde_json::to_string(test_tmp.path().to_str().unwrap()).unwrap(),
        assemble(path, body),
    );
    let (res, _) = server::engine::execute_stateless(&source, config);
    done.store(true, Ordering::SeqCst);
    let _ = wd.join();
    let mut bytes = vec![];
    for x in tree.iter().flatten() {
        bytes.extend_from_slice(&x.1)
    }
    drop(tree);
    drop(db);
    let _ = fs::remove_dir_all(tmp);
    if timed.load(Ordering::SeqCst) {
        return Outcome::Timeout;
    }
    if let Err(e) = res {
        let d = e.to_string();
        return if d.starts_with("AssertionError:") {
            Outcome::Assertion(d)
        } else {
            Outcome::Runtime(d)
        };
    }
    let console = String::from_utf8_lossy(&bytes);
    let Some(line) = console.lines().find_map(|x| x.split(SENTINEL).nth(1)) else {
        return Outcome::Missing;
    };
    match serde_json::from_str::<Report>(line.trim()) {
        Ok(r) if !r.failures.is_empty() => Outcome::Assertion(r.failures.join("\n")),
        Ok(r) if r.skipped.is_some() => Outcome::Skip(r.skipped.unwrap()),
        Ok(_) => Outcome::Pass,
        Err(e) => Outcome::Invalid(e.to_string()),
    }
}
fn path_env(n: &str) -> Result<PathBuf, String> {
    std::env::var_os(n)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{n} required"))
}
fn num_env(n: &str) -> Result<usize, String> {
    std::env::var(n)
        .map_err(|_| format!("{n} required"))?
        .parse()
        .map_err(|_| format!("{n} integer required"))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--node-compat-cli") {
        return node_compat_cli_main(&args[1..]).map_err(Into::into);
    }
    shard_main()
}

fn shard_main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let inv = std::env::var_os("NODE_COMPAT_INVENTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("server/tests/node_compat/inventory.json"));
    let corpus = fs::canonicalize(path_env("NODE_COMPAT_CORPUS")?)?;
    let node_executable = std::env::current_exe()?;
    let results = path_env("NODE_COMPAT_RESULTS")?;
    let summary = path_env("NODE_COMPAT_SUMMARY")?;
    let i = num_env("NODE_COMPAT_SHARD_INDEX")?;
    let n = num_env("NODE_COMPAT_SHARD_TOTAL")?;
    if n == 0 || i >= n {
        return Err("invalid shard".into());
    }
    let timeout = Duration::from_secs(
        std::env::var("NODE_COMPAT_TIMEOUT_SECONDS")
            .ok()
            .and_then(|x| x.parse().ok())
            .unwrap_or(10),
    );
    let inventory: Inventory = serde_json::from_str(&fs::read_to_string(inv)?)?;
    let modules = corpus_modules(&corpus)?;
    if let Some(p) = results.parent() {
        fs::create_dir_all(p)?
    }
    let mut out = BufWriter::new(
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&results)?,
    );
    let mut sum = ShardSummary::new(
        i,
        n,
        &inventory.source.commit,
        &inventory.source.node_version,
    );
    for t in inventory
        .tests
        .iter()
        .filter(|t| shard::stable_shard(&t.path, n) == i)
    {
        let start = Instant::now();
        let (status, reason, details) = match fs::read_to_string(corpus.join(&t.path)) {
            Ok(src) => match run(&t.path, &src, timeout, &modules, &corpus, &node_executable) {
                Outcome::Pass => (ResultStatus::Pass, None, None),
                Outcome::Skip(r) if result::is_platform_inapplicable(&r) => {
                    (ResultStatus::PlatformInapplicable, Some(r), None)
                }
                Outcome::Skip(r) => (ResultStatus::Unsupported, Some(r), None),
                Outcome::Assertion(d) => (ResultStatus::AssertionFailure, None, Some(d)),
                Outcome::Runtime(d) => (ResultStatus::RuntimeError, None, Some(d)),
                Outcome::Timeout => (
                    ResultStatus::Timeout,
                    Some(format!("exceeded {} seconds", timeout.as_secs())),
                    None,
                ),
                Outcome::Missing => (
                    ResultStatus::HarnessMissing,
                    Some("no result sentinel".into()),
                    None,
                ),
                Outcome::Invalid(d) => (
                    ResultStatus::InfrastructureError,
                    Some("invalid report".into()),
                    Some(d),
                ),
            },
            Err(e) => (ResultStatus::FixtureMissing, Some(e.to_string()), None),
        };
        let r = BroadResult::new(
            t,
            &inventory.source,
            i,
            n,
            status,
            start.elapsed(),
            reason,
            details,
        );
        sum.record(&r);
        serde_json::to_writer(&mut out, &r)?;
        out.write_all(b"\n")?;
        out.flush()?
    }
    fs::write(summary, serde_json::to_string_pretty(&sum)? + "\n")?;
    println!(
        "node-compat-full shard {i}/{n}: {} results, {} failing",
        sum.total, sum.failing
    );
    if sum.failing > 0 {
        std::process::exit(1)
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn node_cli_args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn node_cli_parses_repeated_cli_preloads_and_equals_flags() {
        let invocation = NodeCliInvocation::parse(
            &node_cli_args(&[
                "--require=cli-first.cjs",
                "-r",
                "cli-second.cjs",
                "--import=cli-first.mjs",
                "--import",
                "cli-second.mjs",
                "--eval=console.log('ok')",
                "--input-type=module",
                "--no-warnings",
            ]),
            None,
        )
        .unwrap();

        assert_eq!(invocation.environment_requires, Vec::<String>::new());
        assert_eq!(invocation.environment_imports, Vec::<String>::new());
        assert_eq!(invocation.cli_requires, ["cli-first.cjs", "cli-second.cjs"]);
        assert_eq!(invocation.cli_imports, ["cli-first.mjs", "cli-second.mjs"]);
        assert_eq!(invocation.eval_source.as_deref(), Some("console.log('ok')"));
        assert_eq!(invocation.input_type.as_deref(), Some("module"));
        assert!(!invocation.check);
        assert!(invocation.no_warnings);
        assert_eq!(invocation.entrypoint, None);
        assert_eq!(invocation.script_args, Vec::<String>::new());
    }

    #[test]
    fn node_cli_parses_quoted_node_options_before_cli_preloads() {
        let invocation = NodeCliInvocation::parse(
            &node_cli_args(&[
                "--require=cli-require.cjs",
                "--import",
                "cli-import.mjs",
                "entry.mjs",
                "--script-flag",
            ]),
            Some(r#"--require env-require.cjs --import "env import.mjs" -r=env-second.cjs"#),
        )
        .unwrap();

        assert_eq!(
            invocation.environment_requires,
            ["env-require.cjs", "env-second.cjs"]
        );
        assert_eq!(invocation.environment_imports, ["env import.mjs"]);
        assert_eq!(invocation.cli_requires, ["cli-require.cjs"]);
        assert_eq!(invocation.cli_imports, ["cli-import.mjs"]);
        assert_eq!(invocation.entrypoint.as_deref(), Some("entry.mjs"));
        assert_eq!(invocation.script_args, ["--script-flag"]);
    }

    #[test]
    fn node_cli_parses_repeated_experimental_loaders() {
        let invocation = NodeCliInvocation::parse(
            &node_cli_args(&[
                "--experimental-loader",
                "first.mjs",
                "--loader=second.mjs",
                "entry.mjs",
            ]),
            None,
        )
        .unwrap();

        assert_eq!(invocation.experimental_loaders, ["first.mjs", "second.mjs"]);
        assert_eq!(invocation.entrypoint.as_deref(), Some("entry.mjs"));
    }

    #[test]
    fn node_cli_accepts_short_eval_alias() {
        let invocation =
            NodeCliInvocation::parse(&node_cli_args(&["-e", "await import('preload.mjs')"]), None)
                .unwrap();

        assert_eq!(
            invocation.eval_source.as_deref(),
            Some("await import('preload.mjs')")
        );
        assert!(!invocation.check);
    }

    #[test]
    fn node_cli_accepts_short_check_alias() {
        let invocation =
            NodeCliInvocation::parse(&node_cli_args(&["-c", "entry.mjs"]), None).unwrap();

        assert!(invocation.check);
        assert_eq!(invocation.entrypoint.as_deref(), Some("entry.mjs"));
    }

    #[test]
    fn node_cli_double_dash_allows_dash_prefixed_entrypoint() {
        let invocation = NodeCliInvocation::parse(
            &node_cli_args(&["--", "-entry.mjs", "first", "--script-flag"]),
            None,
        )
        .unwrap();

        assert_eq!(invocation.entrypoint.as_deref(), Some("-entry.mjs"));
        assert_eq!(invocation.script_args, ["first", "--script-flag"]);
    }

    #[test]
    fn node_cli_value_flags_reject_following_option_tokens() {
        for args in [
            ["--require", "--no-warnings"],
            ["--import", "--no-warnings"],
            ["--eval", "--no-warnings"],
            ["--input-type", "--no-warnings"],
        ] {
            let error = NodeCliInvocation::parse(&node_cli_args(&args), None).unwrap_err();
            assert!(error.contains("requires a value"), "{args:?}: {error}");
        }
    }

    #[test]
    fn node_cli_rejects_eval_and_check_in_node_options() {
        for node_options in [
            "--eval console.log(1)",
            "-e console.log(1)",
            "--check",
            "-c",
        ] {
            let error = NodeCliInvocation::parse(&[], Some(node_options)).unwrap_err();
            assert!(
                error.contains("not allowed in NODE_OPTIONS"),
                "{node_options}: {error}"
            );
        }
    }

    #[test]
    fn node_cli_rejects_simultaneous_eval_and_check() {
        for args in [
            ["--eval", "console.log(1)", "--check"],
            ["-c", "-e", "console.log(1)"],
        ] {
            let error = NodeCliInvocation::parse(&node_cli_args(&args), None).unwrap_err();
            assert!(
                error.contains("--eval and --check cannot be used together"),
                "{args:?}: {error}"
            );
        }
    }

    #[test]
    fn node_cli_rejects_unsupported_flags() {
        let error = NodeCliInvocation::parse(&node_cli_args(&["--inspect", "entry.mjs"]), None)
            .unwrap_err();

        assert!(error.contains("unsupported Node CLI flag: --inspect"));
    }
    #[test]
    fn node_test_flags_parse_shell_words_and_multiple_directives() {
        assert_eq!(
            test_flags(
                "// Flags: --no-experimental-require-module --title='node test'
\
                 // Flags: --no-warnings
'use strict';
",
            ),
            [
                "--no-experimental-require-module",
                "--title=node test",
                "--no-warnings",
            ],
        );
    }

    #[test]
    fn rewrite_import_meta_resolve_uses_registered_hook_helper() {
        let source = rewrite_import_meta_resolve(
            "file:///fixture.mjs",
            "export const resolved = import.meta.resolve('custom:value');\n",
        )
        .unwrap();

        assert!(
            source.contains("__nodeCompatImportMetaResolve('custom:value')"),
            "{source}"
        );
    }

    #[test]
    fn rewrite_dynamic_imports_uses_loader_helper() {
        let source = rewrite_dynamic_imports("const value = import('./value.json');").unwrap();

        assert!(
            source.contains("__nodeCompatImportWithLoaders('./value.json')"),
            "{source}"
        );
    }

    #[test]
    fn assemble_installs_dynamic_loader_resolve_hooks() {
        let source = assemble(
            "test/es-module/loader.js",
            "// Flags: --experimental-loader ./test/fixtures/loader.mjs\nimport('./value.json');\n",
        );

        assert!(
            source.contains("file:///test/fixtures/loader.mjs"),
            "{source}"
        );
        assert!(
            source.contains("__NODE_COMPAT_IMPORT_WITH_LOADERS__"),
            "{source}"
        );
    }

    #[test]
    fn assemble_runs_test_body_through_commonjs_wrapper() {
        let source = assemble("test/parallel/wrapper.js", "return;");
        let runner = source.split_once(PRELUDE).unwrap().1;
        assert!(runner.contains("globalThis.__NODE_TEST_RUN_CJS__("));
        assert!(runner.contains("globalThis.__NODE_TEST_SCHEDULE_REPORT__("));
        assert!(!runner.contains("(0,eval)("));
    }
    #[test]
    fn assemble_rewrites_import_meta_resolve() {
        let source = assemble(
            "test/es-module/resolve.mjs",
            "import.meta.resolve('custom:value');\n",
        );

        assert!(
            source.contains("__nodeCompatImportMetaResolve('custom:value')"),
            "{source}"
        );
    }

    #[test]
    fn assemble_runs_esm_body_as_a_module() {
        let source = assemble(
            "test/es-module/example.mjs",
            "import '../common/index.mjs';\nimport assert from 'assert';",
        );
        assert!(source.starts_with("import 'node-test:prelude';"));
        assert!(source.contains("import 'node-test:common';"));
        assert!(source.contains("import assert from 'assert';"));
        assert!(!source.contains("globalThis.__NODE_TEST_RUN_CJS__("));
        assert_eq!(
            test_module_specifier("test/es-module/example.mjs").unwrap(),
            "file:///test/es-module/example.mjs",
        );
        let root = std::env::temp_dir().join(format!("node-compat-modules-{}", std::process::id()));
        fs::create_dir_all(root.join("test/fixtures")).unwrap();
        fs::write(root.join("test/fixtures/value.mjs"), "export default 42;\n").unwrap();
        fs::write(
            root.join("test/fixtures/value.js"),
            "module.exports = 42;\n",
        )
        .unwrap();
        fs::write(root.join("test/fixtures/value.json"), "{\"value\":42}\n").unwrap();
        fs::write(
            root.join("test/fixtures/ambiguous.js"),
            "export default 'module';\n",
        )
        .unwrap();
        fs::write(root.join("test/fixtures/addon.node"), [0_u8, 1, 2]).unwrap();
        fs::create_dir_all(root.join("test/fixtures/esm")).unwrap();
        fs::write(
            root.join("test/fixtures/esm/package.json"),
            r#"{"type":"module"}"#,
        )
        .unwrap();
        fs::write(
            root.join("test/fixtures/esm/value.js"),
            "export default 7;\n",
        )
        .unwrap();
        let modules = corpus_modules(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(modules.esm.contains_key("node-test:prelude"));
        assert_eq!(
            modules
                .esm
                .get("file:///test/fixtures/value.mjs")
                .map(String::as_str),
            Some("export default 42;\n"),
        );
        assert!(modules.esm["file:///test/fixtures/value.js"].contains("__cjsModule.exports"));
        assert_eq!(
            modules
                .commonjs
                .get("file:///test/fixtures/value.js")
                .map(String::as_str),
            Some(
                "module.exports = 42;
"
            ),
        );
        assert_eq!(
            modules
                .commonjs
                .get("file:///test/fixtures/value.json")
                .map(String::as_str),
            Some(
                "{\"value\":42}
"
            ),
        );
        assert_eq!(
            modules
                .esm
                .get("file:///test/fixtures/ambiguous.js")
                .map(String::as_str),
            Some(
                "export default 'module';
"
            ),
        );
        assert!(
            modules.commonjs["file:///test/fixtures/ambiguous.js"]
                .starts_with("/*mcp-v8-original-esm:")
        );
        assert!(modules.files.contains("file:///test/fixtures/addon.node"));
        assert_eq!(
            modules
                .esm
                .get("file:///test/fixtures/esm/value.js")
                .map(String::as_str),
            Some("export default 7;\n"),
        );
        let common = modules.esm.get("node-test:common").unwrap();
        assert!(common.contains("export const mustCall"));
        assert!(common.contains("globalThis.__NODE_TEST_COMMON__"));
    }
    #[test]
    fn esm_require_transform_emits_commonjs() {
        let output =
            transpile_esm_to_commonjs("file:///fixture.mjs", "export { name } from './dep.mjs';")
                .unwrap();
        assert!(!output.contains("export "), "{output}");
        assert!(
            output.contains("require(\"mcp-v8:esm-import:./dep.mjs\")"),
            "{output}"
        );
    }

    #[test]
    fn commonjs_wrapper_exposes_statically_detected_named_exports() {
        let source = wrap_commonjs("exports.assert = require('assert');\n");

        assert!(source.contains("export { __cjsNamedExport0 as \"assert\" };"));
    }
    #[test]
    fn esm_syntax_detection_accepts_a_byte_order_mark() {
        assert!(source_uses_esm_syntax(
            "file:///fixture.js",
            "\u{feff}export const value = 1;",
        ));
    }
    #[test]
    fn commonjs_wrapper_exposes_reexported_names() {
        let corpus = cli_corpus(&[
            (
                "test/fixtures/main.js",
                "if (global.maybe) module.exports = require('./dep');\n",
            ),
            ("test/fixtures/dep.js", "exports.reexported = true;\n"),
        ]);
        let source = fs::read_to_string(corpus.path().join("test/fixtures/main.js")).unwrap();
        let names = commonjs_export_names_for_path(
            corpus.path(),
            &corpus.path().join("test/fixtures/main.js"),
            &source,
            &mut HashSet::new(),
        );
        let wrapped = wrap_commonjs_with_names(&source, names);

        assert!(wrapped.contains("as \"reexported\""), "{wrapped}");
    }
    #[test]
    fn commonjs_export_analysis_accepts_a_byte_order_mark() {
        assert_eq!(
            commonjs_export_names("\u{feff}exports.value = 1;"),
            ["value"]
        );
    }
    #[test]
    fn node_commonjs_analysis_matches_object_literal_bailout() {
        assert!(
            node_commonjs_analysis("module.exports = { comeOn: 'fhqwhgads' };\n")
                .exports
                .is_empty()
        );
    }
    #[test]
    fn named_commonjs_import_error_matches_node_message() {
        let corpus = cli_corpus(&[
            (
                "test/fixtures/main.mjs",
                "import { comeOn as renamed } from './fail.cjs';\n",
            ),
            (
                "test/fixtures/fail.cjs",
                "module.exports = { comeOn: 'fhqwhgads' };\n",
            ),
        ]);
        let source = fs::read_to_string(corpus.path().join("test/fixtures/main.mjs")).unwrap();
        let error = named_commonjs_import_error(
            corpus.path(),
            &corpus.path().join("test/fixtures/main.mjs"),
            &source,
        )
        .unwrap();

        assert!(
            error.contains("Named export 'comeOn' not found."),
            "{error}"
        );
        assert!(
            error.contains("const { comeOn: renamed } = pkg;\\n"),
            "{error}"
        );
    }
    #[test]
    fn named_commonjs_import_error_resolves_bare_packages() {
        let corpus = cli_corpus(&[
            (
                "test/fixtures/main.mjs",
                "import { comeOn } from 'deep-fail';\n",
            ),
            (
                "test/fixtures/node_modules/deep-fail/package.json",
                r#"{"main":"index.mjs"}"#,
            ),
            (
                "test/fixtures/node_modules/deep-fail/index.js",
                "module.exports = {\n  comeOn: 'fhqwhgads'\n};\n",
            ),
        ]);
        let source = fs::read_to_string(corpus.path().join("test/fixtures/main.mjs")).unwrap();
        let error = named_commonjs_import_error(
            corpus.path(),
            &corpus.path().join("test/fixtures/main.mjs"),
            &source,
        )
        .unwrap();

        assert!(error.contains("requested module 'deep-fail'"), "{error}");
    }

    #[test]
    fn corpus_modules_inject_bare_commonjs_import_errors() {
        let corpus = cli_corpus(&[
            (
                "test/fixtures/main.mjs",
                "import { comeOn } from 'deep-fail';\n",
            ),
            (
                "test/fixtures/node_modules/deep-fail/package.json",
                r#"{"main":"index.mjs"}"#,
            ),
            (
                "test/fixtures/node_modules/deep-fail/index.js",
                "module.exports = {\n  comeOn: 'fhqwhgads'\n};\n",
            ),
        ]);
        let modules = corpus_modules(corpus.path()).unwrap();
        let source = modules.esm.get("file:///test/fixtures/main.mjs").unwrap();

        assert!(source.starts_with("throw new SyntaxError("), "{source}");
    }

    #[test]
    fn named_commonjs_import_error_omits_multiline_destructure() {
        let corpus = cli_corpus(&[
            (
                "test/fixtures/main.mjs",
                "import {\n  comeOn,\n  everybody,\n} from './fail.cjs';\n",
            ),
            (
                "test/fixtures/fail.cjs",
                "module.exports = { comeOn: 'fhqwhgads', everybody: 'limit' };\n",
            ),
        ]);
        let source = fs::read_to_string(corpus.path().join("test/fixtures/main.mjs")).unwrap();
        let error = named_commonjs_import_error(
            corpus.path(),
            &corpus.path().join("test/fixtures/main.mjs"),
            &source,
        )
        .unwrap();

        assert!(!error.contains("const {"), "{error}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn node_test_filesystem_policy_is_isolated() {
        let root = tempfile::tempdir().unwrap();
        let (_policy_dir, policy) = isolated_fs_policy(root.path()).unwrap();
        let allowed = serde_json::json!({
            "operation": "writeFile",
            "path": root.path().join("fixture.mjs"),
        });
        let denied = serde_json::json!({
            "operation": "writeFile",
            "path": "/tmp/outside-node-compat-fixture.mjs",
        });

        assert!(policy.evaluate(&allowed).await.unwrap());
        assert!(!policy.evaluate(&denied).await.unwrap());
    }
    #[test]
    fn platform_skip_is_strict() {
        assert!(result::is_platform_inapplicable("Windows-only"));
        assert!(!result::is_platform_inapplicable("missing crypto"));
        assert!(!result::is_platform_inapplicable(
            "V8 inspector is disabled"
        ));
    }
    #[test]
    fn stable_shards() {
        assert_eq!(
            shard::stable_shard("test/parallel/a.js", 16),
            shard::stable_shard("test/parallel/a.js", 16)
        );
    }
    fn cli_corpus(files: &[(&str, &str)]) -> tempfile::TempDir {
        let corpus = tempfile::tempdir().unwrap();
        fs::create_dir_all(corpus.path().join("test")).unwrap();
        for (path, source) in files {
            let path = corpus.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        corpus
    }

    #[test]
    fn node_cli_virtualizes_corpus_paths_and_rejects_external_paths() {
        let corpus = cli_corpus(&[("test/entry.mjs", "export {};\n")]);
        let host_entry = corpus.path().join("test/entry.mjs");
        assert_eq!(
            virtualize_corpus_path(&host_entry.to_string_lossy(), corpus.path()).unwrap(),
            "/test/entry.mjs"
        );
        assert_eq!(
            virtualize_corpus_path("file:///test/entry.mjs", corpus.path()).unwrap(),
            "file:///test/entry.mjs"
        );
        assert!(virtualize_corpus_path("/outside/entry.mjs", corpus.path()).is_err());
    }

    #[test]
    fn node_cli_indexes_only_reachable_corpus_directories() {
        let corpus = cli_corpus(&[
            (
                "test/fixtures/entry/main.mjs",
                "import './dep.js'; import 'pkg';\n",
            ),
            ("test/fixtures/entry/dep.js", "exports.value = 1;\n"),
            (
                "test/fixtures/node_modules/pkg/index.js",
                "exports.packageValue = 1;\n",
            ),
            (
                "test/fixtures/unrelated/slow.js",
                "exports.unrelated = 1;\n",
            ),
        ]);
        let invocation = NodeCliInvocation::parse(
            &node_cli_args(&[&corpus
                .path()
                .join("test/fixtures/entry/main.mjs")
                .to_string_lossy()]),
            None,
        )
        .unwrap();

        let modules = corpus_modules_for_cli(corpus.path(), &invocation).unwrap();

        assert!(
            modules
                .esm
                .contains_key("file:///test/fixtures/entry/dep.js")
        );
        assert!(
            modules
                .esm
                .contains_key("file:///test/fixtures/node_modules/pkg/index.js")
        );
        assert!(
            !modules
                .esm
                .contains_key("file:///test/fixtures/unrelated/slow.js")
        );
    }

    #[test]
    fn node_cli_runs_preloads_in_node_order_and_reports_mcp_v8_identity() {
        let corpus = cli_corpus(&[
            ("test/env-require.cjs", "console.log('env-require');\n"),
            (
                "test/env-import.mjs",
                "console.log('env-import:start'); await Promise.resolve(); console.log('env-import:end');\n",
            ),
            ("test/cli-require.cjs", "console.log('cli-require');\n"),
            ("test/cli-import.mjs", "console.log('cli-import');\n"),
        ]);
        let output = run_node_compat_cli(
            &node_cli_args(&[
                "--require",
                &corpus.path().join("test/cli-require.cjs").to_string_lossy(),
                "--import",
                &corpus.path().join("test/cli-import.mjs").to_string_lossy(),
                "--eval",
                "console.log(process.versions['mcp-v8']); console.log('eval');",
            ]),
            Some(&format!(
                "--require {} --import {}",
                corpus.path().join("test/env-require.cjs").display(),
                corpus.path().join("test/env-import.mjs").display(),
            )),
            corpus.path(),
        )
        .unwrap();

        assert_eq!(output.runtime_error, None);
        assert_eq!(
            output.stdout.lines().collect::<Vec<_>>(),
            [
                "env-require",
                "cli-require",
                "env-import:start",
                "env-import:end",
                "cli-import",
                "1.0.0",
                "eval",
            ]
        );
    }

    #[test]
    fn node_cli_notes_only_static_esm_syntax_in_commonjs() {
        let corpus = cli_corpus(&[
            ("test/static.cjs", "export const value = 1;\n"),
            ("test/dynamic.cjs", "import('./missing.mjs');\n"),
            ("test/error.cjs", "throw new Error('boom');\n"),
        ]);
        let run = |name: &str| {
            run_node_compat_cli(
                &node_cli_args(&[&corpus.path().join(name).to_string_lossy()]),
                None,
                corpus.path(),
            )
            .unwrap()
        };

        let static_import = run("test/static.cjs");
        assert!(
            static_import
                .stderr
                .contains("Failed to load the ES module"),
            "{}",
            static_import.stderr
        );
        assert!(
            !run("test/dynamic.cjs")
                .stderr
                .contains("Failed to load the ES module")
        );
        assert!(
            !run("test/error.cjs")
                .stderr
                .contains("Failed to load the ES module")
        );
    }

    #[test]
    fn node_cli_exposes_node_global_alias() {
        let corpus = cli_corpus(&[]);
        let output = run_node_compat_cli(
            &node_cli_args(&["--eval", "console.log(global === globalThis);"]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(output.runtime_error, None);
        assert_eq!(output.stdout, "true\n");
    }

    #[test]
    fn node_cli_waits_for_each_import_before_starting_the_next() {
        let corpus = cli_corpus(&[
            (
                "test/first.mjs",
                "console.log('first:start'); await Promise.resolve(); console.log('first:end');\n",
            ),
            ("test/second.mjs", "console.log('second');\n"),
        ]);
        let output = run_node_compat_cli(
            &node_cli_args(&[
                "--import",
                &corpus.path().join("test/first.mjs").to_string_lossy(),
                "--import",
                &corpus.path().join("test/second.mjs").to_string_lossy(),
                "--eval",
                "console.log('eval');",
            ]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(
            output.stdout.lines().collect::<Vec<_>>(),
            ["first:start", "first:end", "second", "eval"]
        );
    }

    #[test]
    fn node_cli_check_evaluates_preloads_and_compiles_ambiguous_esm_without_running_it() {
        let corpus = cli_corpus(&[
            ("test/preload.mjs", "console.log('preload');\n"),
            (
                "test/check.js",
                "export var name = 5; throw new Error('entrypoint evaluated');\n",
            ),
        ]);
        let output = run_node_compat_cli(
            &node_cli_args(&[
                "--import",
                &corpus.path().join("test/preload.mjs").to_string_lossy(),
                "--check",
                &corpus.path().join("test/check.js").to_string_lossy(),
            ]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(output.stdout, "preload\n");
    }

    #[test]
    fn node_cli_check_accepts_ambiguous_commonjs_top_level_return() {
        let corpus = cli_corpus(&[(
            "test/check.js",
            "return; throw new Error('entrypoint evaluated');\n",
        )]);
        let output = run_node_compat_cli(
            &node_cli_args(&[
                "--check",
                &corpus.path().join("test/check.js").to_string_lossy(),
            ]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(output.runtime_error, None);
    }

    #[test]
    fn node_cli_check_compiles_esm_without_resolving_imports() {
        let corpus = cli_corpus(&[(
            "test/check.js",
            "import './missing.mjs'; export const checked = true;\n",
        )]);
        let output = run_node_compat_cli(
            &node_cli_args(&[
                "--check",
                &corpus.path().join("test/check.js").to_string_lossy(),
            ]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(output.runtime_error, None);
    }

    #[test]
    fn node_cli_separates_stderr_and_preserves_process_exit_code() {
        let corpus = cli_corpus(&[]);
        let output = run_node_compat_cli(
            &node_cli_args(&[
                "--eval",
                "console.log('stdout'); console.error('stderr'); process.exit(7); console.log('after');",
            ]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(output.stdout, "stdout\n");
        assert_eq!(output.stderr, "stderr\n");
        assert_eq!(output.exit_code, 7);
        assert_eq!(output.runtime_error, None);
    }

    #[test]
    fn node_cli_applies_process_invocation_state_and_script_args() {
        let corpus = cli_corpus(&[(
            "test/argv.mjs",
            "console.log(JSON.stringify({ argv: process.argv, execArgv: process.execArgv, env: process.env.TMPDIR, execPath: process.execPath, cwd: process.cwd() }));\n",
        )]);
        let output = run_node_compat_cli(
            &node_cli_args(&[
                "--no-warnings",
                &corpus.path().join("test/argv.mjs").to_string_lossy(),
                "first",
                "--script-flag",
            ]),
            None,
            corpus.path(),
        )
        .unwrap();
        let state: serde_json::Value = serde_json::from_str(output.stdout.trim()).unwrap();

        assert_eq!(
            state["argv"],
            serde_json::json!([
                std::env::current_exe().unwrap(),
                "/test/argv.mjs",
                "first",
                "--script-flag"
            ])
        );
        assert_eq!(state["execArgv"], serde_json::json!(["--no-warnings"]));
        assert_eq!(state["env"], "/dev/shm");
        assert_eq!(
            state["execPath"],
            std::env::current_exe().unwrap().to_string_lossy().as_ref()
        );
        assert_eq!(
            state["cwd"],
            std::env::current_dir().unwrap().to_string_lossy().as_ref()
        );
    }

    #[test]
    fn node_cli_honors_commonjs_and_module_input_types() {
        let corpus = cli_corpus(&[]);
        let commonjs = run_node_compat_cli(
            &node_cli_args(&[
                "--input-type=commonjs",
                "--eval",
                "console.log(typeof require); return;",
            ]),
            None,
            corpus.path(),
        )
        .unwrap();
        let module = run_node_compat_cli(
            &node_cli_args(&[
                "--input-type=module",
                "--eval",
                "await Promise.resolve(); console.log(typeof import.meta.url);",
            ]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(commonjs.runtime_error, None);
        assert_eq!(commonjs.stdout, "function\n");
        assert_eq!(module.runtime_error, None);
        assert_eq!(module.stdout, "string\n");
    }

    #[test]
    fn node_cli_uses_process_exit_code_on_normal_completion() {
        let corpus = cli_corpus(&[]);
        let output = run_node_compat_cli(
            &node_cli_args(&["--eval", "process.exitCode = 9;"]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(output.exit_code, 9);
        assert_eq!(output.runtime_error, None);
    }

    #[test]
    fn node_cli_process_exit_cannot_be_caught() {
        let corpus = cli_corpus(&[]);
        let output = run_node_compat_cli(
            &node_cli_args(&[
                "--eval",
                "try { process.exit(7); } catch { console.log('caught'); } console.log('after');",
            ]),
            None,
            corpus.path(),
        )
        .unwrap();

        assert_eq!(output.stdout, "");
        assert_eq!(output.exit_code, 7);
        assert_eq!(output.runtime_error, None);
    }

    #[test]
    fn node_cli_preserves_virtual_argv_path_and_exact_exec_argv() {
        let corpus = cli_corpus(&[
            ("test/env-require.cjs", ""),
            ("test/env-import.mjs", ""),
            ("test/cli-require.cjs", ""),
            ("test/cli-import.mjs", ""),
            (
                "test/argv.mjs",
                "console.log(JSON.stringify({ argv: process.argv, execArgv: process.execArgv }));\n",
            ),
        ]);
        let args = node_cli_args(&[
            "--import=/test/cli-import.mjs",
            "-r",
            "/test/cli-require.cjs",
            "--no-warnings",
            "/test/argv.mjs",
            "script-arg",
        ]);
        let invocation = NodeCliInvocation::parse(&args, None).unwrap();
        assert_eq!(
            invocation.exec_argv,
            [
                "--import=/test/cli-import.mjs",
                "-r",
                "/test/cli-require.cjs",
                "--no-warnings",
            ]
        );

        let output = run_node_compat_cli(&args, None, corpus.path()).unwrap();
        let state: serde_json::Value = serde_json::from_str(output.stdout.trim()).unwrap();
        assert_eq!(state["argv"][1], "/test/argv.mjs");
        assert_eq!(
            state["execArgv"],
            serde_json::json!([
                "--import=/test/cli-import.mjs",
                "-r",
                "/test/cli-require.cjs",
                "--no-warnings",
            ])
        );
    }
}
