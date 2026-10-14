import importlib.util
from pathlib import Path


def require(condition, detail):
    if not condition:
        raise ValueError(str(detail))


p = Path.cwd() / 'scripts/check_docs_formatted.py'
spec = importlib.util.spec_from_file_location('check_docs', p)
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
calls = []
def formatter(code, extension):
    calls.append((code, extension))
    return 'FORMATTED\n'
m.format_str = formatter
for language, extension in [('python3', 'py'), ('python', 'py'), ('pyi', 'pyi')]:
    calls.clear()
    actual, errors = m.format_contents(f'  ```{language}\n  x=1\n  ```\n')
    require(calls == [('x=1\n', extension)], (language, calls))
    require('  FORMATTED\n' in actual and not errors, (actual, errors))
for language in ['javascript', 'python2', '']:
    source = f'```{language}\nx=1\n```\n'
    calls.clear()
    require(m.format_contents(source) == (source, []), language)
    require(calls == [], calls)
def invalid(code, extension):
    raise m.InvalidInput('error: Failed to parse')
m.format_str = invalid
actual, errors = m.format_contents('```python3\nbroken\n```\n')
require(len(errors) == 1 and 'broken' in actual, (actual, errors))
print('PASS: aliases, stub extension, indentation, unchanged languages, parse errors')
