from golem import agent, method


@agent
class CounterAgent:
    """A durable counter. Golem keeps its state between calls."""

    def __init__(self, name: str) -> None:
        self.name = name
        self.count = 0

    @method
    def increment(self) -> int:
        self.count += 1
        return self.count

    @method
    def add(self, by: int) -> int:
        self.count += by
        return self.count

    @method
    def value(self) -> int:
        return self.count

    @method
    def reset(self) -> None:
        self.count = 0
