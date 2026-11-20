"""Native tests of agent declarations and dispatch.

Run with CPython 3.14 from `sdks/python`:

    python -m unittest discover -s tests
"""

import sys
import textwrap
import types
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from golem import _registry
from golem._dispatch import (
    Dispatcher,
    Failure,
    InvalidInput,
    InvalidMethod,
    InvalidType,
)
from golem._registry import BINARY, BOOL, F64, S64, STRING, DeclarationError


def load(source: str, name: str = "app") -> types.ModuleType:
    """Executes `source` as module `name` with a fresh registry."""
    _registry.current = _registry.Registry()
    module = types.ModuleType(name)
    module.__file__ = f"{name}.py"
    code = compile(textwrap.dedent(source), module.__file__, "exec")
    exec(code, module.__dict__)
    return module


COUNTER = """
    from golem import agent, method

    CONSTRUCTED = []

    @agent
    class Counter:
        def __init__(self, name: str) -> None:
            CONSTRUCTED.append(name)
            self.name = name
            self.value = 0

        @method
        def increment(self, amount: Amount) -> int:
            self.value += amount
            return self.value

        @method()
        def reset(self) -> None:
            self.value = 0

        @method
        def describe(self, loud: bool, ratio: float, data: bytes) -> str:
            return f"{self.name}:{self.value}:{loud}:{ratio}:{data.hex()}"

        @method
        def bad_bool(self) -> int:
            self.value += 100
            return True

        @method
        def bad_float(self) -> float:
            return 1

        @method
        def big(self, offset: int) -> int:
            return 2**63 + offset

        @method
        def small(self, offset: int) -> int:
            return -(2**63) + offset

        @method
        def not_none(self) -> None:
            return 0

        @method
        def fails(self) -> str:
            self.value += 1000
            raise ValueError("boom")

        def helper(self) -> int:
            return 0

    # Defined after the class: annotations resolve when the registry finalizes.
    Amount = int
"""


def arguments(*values):
    return lambda parameters: list(values)


class Declarations(unittest.TestCase):
    def test_counter(self):
        module = load(COUNTER)
        [counter] = _registry.current.finalize()

        self.assertEqual(module.CONSTRUCTED, [])
        self.assertEqual(counter.name, "Counter")
        self.assertIs(counter.cls, module.Counter)
        self.assertEqual(
            [(p.name, p.kind) for p in counter.parameters], [("name", STRING)]
        )
        self.assertEqual(
            list(counter.methods),
            [
                "increment",
                "reset",
                "describe",
                "bad_bool",
                "bad_float",
                "big",
                "small",
                "not_none",
                "fails",
            ],
        )
        increment = counter.methods["increment"]
        self.assertEqual(
            [(p.name, p.kind) for p in increment.parameters], [("amount", S64)]
        )
        self.assertIs(increment.output, S64)
        self.assertIsNone(counter.methods["reset"].output)
        describe = counter.methods["describe"]
        self.assertEqual([p.kind for p in describe.parameters], [BOOL, F64, BINARY])
        self.assertIs(describe.output, STRING)

    def test_finalizes_once(self):
        load(COUNTER)
        _registry.current.finalize()
        with self.assertRaisesRegex(RuntimeError, "already finalized"):
            _registry.current.finalize()

    def test_reports_every_invalid_declaration(self):
        source = """
            from golem import agent, method

            @agent
            class Lists:
                def __init__(self, items: list[int]) -> None:
                    pass

            @agent
            class Shapes:
                @method
                async def later(self) -> int:
                    return 1

                @method
                def untyped(self, value) -> int:
                    return value

                @method
                def defaulted(self, value: int = 1) -> int:
                    return value

                @method
                def variadic(self, *values: int) -> int:
                    return 0

                @method
                def keyword(self, *, value: int) -> int:
                    return value

                @method
                def unannotated_return(self):
                    return 0

                @method
                def returns_dict(self) -> dict:
                    return {}

                @method
                def unresolved(self, value: Missing) -> int:
                    return 0

                @staticmethod
                @method
                def static(value: int) -> int:
                    return value

            @agent
            class Configured:
                config: int

            class Plain:
                @method
                def orphan(self) -> int:
                    return 0

            @agent
            class Lists:
                pass

            @agent
            def not_a_class():
                pass
        """
        load(source)
        with self.assertRaises(DeclarationError) as raised:
            _registry.current.finalize()
        errors = raised.exception.errors
        expected = [
            "app.py:6: parameter `items` of the constructor of `Lists` has the unsupported type `list[int]`",
            "app.py:11: method `Shapes.later` is async",
            "app.py:15: parameter `value` of method `Shapes.untyped` has no type annotation",
            "app.py:19: parameter `value` of method `Shapes.defaulted` has a default value",
            "app.py:23: parameter `values` of method `Shapes.variadic` is variadic",
            "app.py:27: parameter `value` of method `Shapes.keyword` is keyword-only",
            "app.py:31: method `Shapes.unannotated_return` has no return type annotation",
            "app.py:35: method `Shapes.returns_dict` returns the unsupported type `dict`; supported types are bool, int, float, str, bytes and None",
            "app.py:39: cannot resolve the annotations of method `Shapes.unresolved`: NameError",
            "app.py:43: method `Shapes.static` must be an instance method, not a staticmethod",
            "app.py:48: agent `Configured` declares `config`",
            "app.py:53: `Plain.orphan` is decorated with @method, but its class is not decorated with @agent",
            "app.py:61: @agent must decorate a class, not function",
        ]
        for prefix in expected:
            self.assertTrue(
                any(error.startswith(prefix) for error in errors),
                f"missing {prefix!r} in {errors}",
            )
        # The second `Lists` is valid, but the first is not, so it is the only
        # declaration with that name.
        self.assertEqual(len(errors), len(expected), errors)
        self.assertIn(
            f"{len(expected)} invalid agent declarations", str(raised.exception)
        )

    def test_annotation_wrappers_are_unsupported(self):
        load(
            """
            from typing import Annotated
            from golem import agent, method

            @agent
            class Wrapped:
                def __init__(self, name: str) -> None:
                    self.name = name

                @method
                def takes(self, value: Annotated[int, "units"]) -> int:
                    return value

                @method
                def gives(self, value: int) -> Annotated[int, "units"]:
                    return value

                @method
                def plain(self, value: int) -> int:
                    return value
            """
        )
        with self.assertRaises(DeclarationError) as raised:
            _registry.current.finalize()
        self.assertEqual(
            raised.exception.errors,
            [
                (
                    "app.py:10: parameter `value` of method `Wrapped.takes` has the "
                    "unsupported type `typing.Annotated[int, 'units']`; supported types "
                    "are bool, int, float, str and bytes"
                ),
                (
                    "app.py:14: method `Wrapped.gives` returns the unsupported type "
                    "`typing.Annotated[int, 'units']`; supported types are bool, int, "
                    "float, str, bytes and None"
                ),
            ],
        )

    def test_annotation_wrappers_with_unhashable_metadata_are_unsupported(self):
        load(
            """
            from typing import Annotated
            from golem import agent, method

            @agent
            class Wrapped:
                def __init__(self, name: str) -> None:
                    self.name = name

                @method
                def takes(self, value: Annotated[int, {}]) -> int:
                    return value

                @method
                def gives(self, value: int) -> Annotated[int, []]:
                    return value

                @method
                def plain(self, value: int) -> int:
                    return value

            @agent
            class Lists:
                def __init__(self, items: list[int]) -> None:
                    pass
            """
        )
        with self.assertRaises(DeclarationError) as raised:
            _registry.current.finalize()
        self.assertEqual(
            raised.exception.errors,
            [
                (
                    "app.py:10: parameter `value` of method `Wrapped.takes` has the "
                    "unsupported type `typing.Annotated[int, {}]`; supported types "
                    "are bool, int, float, str and bytes"
                ),
                (
                    "app.py:14: method `Wrapped.gives` returns the unsupported type "
                    "`typing.Annotated[int, []]`; supported types are bool, int, "
                    "float, str, bytes and None"
                ),
                (
                    "app.py:24: parameter `items` of the constructor of `Lists` has "
                    "the unsupported type `list[int]`; supported types are bool, int, "
                    "float, str and bytes"
                ),
            ],
        )

    def test_methods_take_a_positional_receiver(self):
        load(
            """
            from golem import agent, method

            @agent
            class Receivers:
                @method
                def keyword(*, value: int) -> int:
                    return value

                @method
                def star(*args: int) -> int:
                    return 0

                @method
                def double_star(**kwargs: int) -> int:
                    return 0

                @method
                def nothing() -> int:
                    return 0

            @agent
            class Ctor:
                def __init__(*, name: str) -> None:
                    pass
            """
        )
        with self.assertRaises(DeclarationError) as raised:
            _registry.current.finalize()
        receiver = "must take the agent instance as its first positional parameter"
        self.assertEqual(
            raised.exception.errors,
            [
                f"app.py:6: method `Receivers.keyword` {receiver}",
                f"app.py:10: method `Receivers.star` {receiver}",
                f"app.py:14: method `Receivers.double_star` {receiver}",
                f"app.py:18: method `Receivers.nothing` {receiver}",
                f"app.py:24: the constructor of `Ctor` {receiver}",
            ],
        )

        # The receiver needs no particular name and may be positional-only.
        load(
            """
            from golem import agent, method

            @agent
            class Receivers:
                def __init__(this, start: int) -> None:
                    this.start = start

                @method
                def ordinary(this, value: int) -> int:
                    return this.start + value

                @method
                def positional_only(this, /, value: int) -> int:
                    return this.start - value
            """
        )
        dispatcher = Dispatcher(_registry.current.finalize())
        dispatcher.initialize("Receivers", arguments(10))
        self.assertEqual(dispatcher.invoke("ordinary", arguments(3)), (S64, 13))
        self.assertEqual(dispatcher.invoke("positional_only", arguments(3)), (S64, 7))

    def test_class_annotation_errors_do_not_stop_finalization(self):
        load(
            """
            from golem import agent, method

            @agent
            class Broken:
                config: 1 / 0

            @agent
            class Lists:
                def __init__(self, items: list[int]) -> None:
                    pass

            @agent
            class Fine:
                @method
                def value(self) -> int:
                    return 1
            """
        )
        with self.assertRaises(DeclarationError) as raised:
            _registry.current.finalize()
        self.assertEqual(
            raised.exception.errors,
            [
                (
                    "app.py:4: cannot evaluate the class annotations of agent `Broken`: "
                    "ZeroDivisionError: division by zero"
                ),
                (
                    "app.py:10: parameter `items` of the constructor of `Lists` has the "
                    "unsupported type `list[int]`; supported types are bool, int, float, "
                    "str and bytes"
                ),
            ],
        )

    def test_duplicate_agent_types(self):
        load(
            """
            from golem import agent

            @agent
            class Twice:
                pass

            @agent
            class Twice:
                pass
            """
        )
        with self.assertRaises(DeclarationError) as raised:
            _registry.current.finalize()
        self.assertEqual(
            raised.exception.errors,
            ["app.py:8: agent type `Twice` is already declared at app.py:4"],
        )

    def test_constructor_cannot_be_a_method(self):
        load(
            """
            from golem import agent, method

            @agent
            class Odd:
                @method
                def __init__(self) -> None:
                    pass
            """
        )
        with self.assertRaises(DeclarationError) as raised:
            _registry.current.finalize()
        self.assertEqual(len(raised.exception.errors), 1)
        self.assertIn("cannot be decorated with @method", raised.exception.errors[0])


class Dispatch(unittest.TestCase):
    def setUp(self):
        self.module = load(COUNTER)
        self.dispatcher = Dispatcher(_registry.current.finalize())

    def initialize(self, name="alice"):
        self.dispatcher.initialize("Counter", arguments(name))

    def call(self, method, *values):
        return self.dispatcher.invoke(method, arguments(*values))

    def test_keeps_state_between_calls(self):
        self.initialize()
        self.assertEqual(self.call("increment", 3), (S64, 3))
        self.assertEqual(self.call("increment", 7), (S64, 10))
        self.assertEqual(
            self.call("describe", False, 0.5, b"\x00\xff"),
            (STRING, "alice:10:False:0.5:00ff"),
        )
        self.assertEqual(self.call("reset"), (None, None))
        self.assertEqual(self.call("increment", -2), (S64, -2))
        self.assertEqual(self.module.CONSTRUCTED, ["alice"])

    def test_initializes_once(self):
        self.initialize("alice")
        self.call("increment", 5)
        with self.assertRaisesRegex(Failure, "already initialized as `Counter`"):
            self.initialize("bob")
        self.assertEqual(self.call("increment", 1), (S64, 6))
        self.assertEqual(self.module.CONSTRUCTED, ["alice"])

    def test_rejects_unknown_types_and_methods(self):
        with self.assertRaisesRegex(Failure, "not initialized"):
            self.call("increment", 1)
        with self.assertRaisesRegex(
            InvalidType, "no agent type `Missing`; it defines `Counter`"
        ):
            self.dispatcher.initialize("Missing", arguments())
        self.initialize()
        with self.assertRaisesRegex(InvalidMethod, "no method `helper`"):
            self.call("helper")
        self.assertEqual(self.module.CONSTRUCTED, ["alice"])

    def test_invalid_input_runs_no_user_code(self):
        def reject(parameters):
            raise InvalidInput("bad")

        with self.assertRaises(InvalidInput):
            self.dispatcher.initialize("Counter", reject)
        self.assertEqual(self.module.CONSTRUCTED, [])
        self.initialize()
        self.call("increment", 4)
        with self.assertRaises(InvalidInput):
            self.dispatcher.invoke("increment", reject)
        self.assertEqual(self.call("increment", 0), (S64, 4))

    def test_wrong_results_fail_after_the_method_runs(self):
        self.initialize()
        with self.assertRaisesRegex(
            Failure, "`bad_bool` returned an invalid result: expected int, got bool"
        ):
            self.call("bad_bool")
        # The method's effect remains.
        self.assertEqual(self.call("increment", 0), (S64, 100))
        with self.assertRaisesRegex(Failure, "expected float, got int"):
            self.call("bad_float")
        with self.assertRaisesRegex(Failure, "expected None, got int"):
            self.call("not_none")

    def test_int_results_are_signed_64_bit(self):
        self.initialize()
        self.assertEqual(self.call("big", -1), (S64, 2**63 - 1))
        self.assertEqual(self.call("small", 0), (S64, -(2**63)))
        with self.assertRaisesRegex(Failure, f"{2**63} is out of the range"):
            self.call("big", 0)
        with self.assertRaisesRegex(Failure, f"{-(2**63) - 1} is out of the range"):
            self.call("small", -1)

    def test_exceptions_fail_the_call(self):
        self.initialize()
        with self.assertRaisesRegex(Failure, "`fails` failed: ValueError: boom"):
            self.call("fails")
        self.assertEqual(self.call("increment", 0), (S64, 1000))

    def test_failed_constructor_leaves_the_agent_uninitialized(self):
        load(
            """
            from golem import agent

            @agent
            class Picky:
                def __init__(self, name: str) -> None:
                    if not name:
                        raise ValueError("empty name")
                    self.name = name
            """
        )
        dispatcher = Dispatcher(_registry.current.finalize())
        with self.assertRaisesRegex(
            Failure, "the constructor of `Picky` failed: ValueError: empty name"
        ):
            dispatcher.initialize("Picky", arguments(""))
        self.assertIsNone(dispatcher.declaration)
        dispatcher.initialize("Picky", arguments("ok"))
        self.assertEqual(dispatcher.declaration.name, "Picky")


if __name__ == "__main__":
    unittest.main()
