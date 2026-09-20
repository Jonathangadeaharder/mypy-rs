def use(d: D) -> int:
    return d.v


class A:
    def m(self, d: D) -> int:
        return d.v


class D:
    def __init__(self) -> None:
        self.v = 1
