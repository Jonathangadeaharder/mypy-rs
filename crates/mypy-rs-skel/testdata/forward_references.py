def first() -> int:
    return second()


def second() -> int:
    return 1


class C:
    def reader(self) -> int:
        return self.value

    def writer(self) -> None:
        self.value = 1


class D:
    def caller(self) -> int:
        return later()


def later() -> int:
    return 2


def global_reader() -> int:
    return counter


counter: int = 3
