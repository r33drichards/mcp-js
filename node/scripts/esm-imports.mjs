import ts from 'typescript';

export function esmImports(text, name) {
  const source = ts.createSourceFile(name, text, ts.ScriptTarget.Latest, true);
  const edits = [];
  function visit(node) {
    const specifier = (ts.isImportDeclaration(node) || ts.isExportDeclaration(node))
      ? node.moduleSpecifier
      : ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword
        ? node.arguments[0] : undefined;
    if (specifier && ts.isStringLiteral(specifier) && /^\.\.?\//.test(specifier.text) && !/\.[a-z]+$/i.test(specifier.text)) {
      edits.push(specifier.end - 1);
    }
    ts.forEachChild(node, visit);
  }
  visit(source);
  for (const position of edits.sort((a, b) => b - a)) text = text.slice(0, position) + '.js' + text.slice(position);
  return text;
}
