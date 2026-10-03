// Test-only syntax inspection of the actual served renderer asset. No runtime
// compatibility shims, operation registries or state substitutes are installed.
const fs = require('node:fs');
const path = require('node:path');
const parser = require(path.resolve(process.argv[2], 'web/node_modules/@babel/parser'));
const source = fs.readFileSync(process.argv[3], 'utf8');
const tree = parser.parse(source, {sourceType: 'unambiguous'});
const candidates = [];
function walk(node, functions = []) {
  if (!node || typeof node !== 'object') return;
  if (Array.isArray(node)) {for (const child of node) walk(child, functions); return;}
  if (['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression'].includes(node.type))
    functions = [...functions, node];
  if (node.type === 'ForStatement' && node.update?.type === 'AssignmentExpression'
      && node.update.operator === '+=' && node.update.right?.value === 250) {
    const literals = []; let awaits = 0, slices = 0;
    function inspect(child) {
      if (!child || typeof child !== 'object') return;
      if (Array.isArray(child)) {child.forEach(inspect); return;}
      if (child.type === 'NumericLiteral' && child.value === 250) literals.push([child.start, child.end]);
      if (child.type === 'AwaitExpression') awaits++;
      if (child.type === 'MemberExpression' && child.property?.name === 'slice') slices++;
      for (const [key, value] of Object.entries(child)) if (!['loc', 'extra', 'comments', 'tokens'].includes(key)) inspect(value);
    }
    inspect(node);
    const owner = functions.at(-1);
    // The actual renderer has four uses of its batch size and one Promise
    // yield. Identify its returned named renderSession method as well.
    if (literals.length === 4 && awaits === 1 && slices === 1 && owner?.id?.name) {
      const name = owner.id.name;
      let exported = false;
      function exportedMethod(child) {
        if (!child || typeof child !== 'object') return;
        if (Array.isArray(child)) {child.forEach(exportedMethod); return;}
        if (child.type === 'ObjectProperty' && (child.key?.name || child.key?.value) === 'renderSession'
            && child.value?.type === 'Identifier' && child.value.name === name) exported = true;
        for (const [key, value] of Object.entries(child)) if (!['loc', 'extra', 'comments', 'tokens'].includes(key)) exportedMethod(value);
      }
      exportedMethod(tree.program);
      if (exported) candidates.push({name, literals});
    }
  }
  for (const [key, value] of Object.entries(node)) if (!['loc', 'extra', 'comments', 'tokens'].includes(key)) walk(value, functions);
}
walk(tree.program);
process.stdout.write(JSON.stringify(candidates));
