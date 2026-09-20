class A:
    def __init__(self) -> None:
        pass


class B(A):
    def __init__(self) -> None:
        pass


class Base:
    def val(self) -> A:
        return A()

    def __init__(self) -> None:
        pass


class Derived(Base):
    def val(self) -> B:
        return B()

    def __init__(self) -> None:
        self.x = self.val()

    def probe(self) -> B:
        return self.x
