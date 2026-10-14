"""Handler registry: names in jobs map to Python callables."""

import importlib

from .errors import UnknownHandler


class Registry:
    def __init__(self):
        self._handlers = {}

    def register(self, name: str, function) -> None:
        if name in self._handlers and self._handlers[name] is not function:
            raise ValueError(f"handler {name!r} is already registered")
        self._handlers[name] = function

    def get(self, name: str):
        try:
            return self._handlers[name]
        except KeyError:
            raise UnknownHandler(f"no handler named {name!r}") from None

    def names(self) -> list[str]:
        return sorted(self._handlers)

    def task(self, name: str | None = None):
        """Decorator: ``@registry.task()`` registers a function under its own name."""

        def decorate(function):
            self.register(name or function.__name__, function)
            return function

        return decorate

    def load(self, module: str) -> None:
        """Import ``module`` so its ``@task`` decorators run."""
        importlib.import_module(module)


default_registry = Registry()
task = default_registry.task
