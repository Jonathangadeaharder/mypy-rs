class A:
    def __init__(self) -> None:
        pass


class B(A):
    def __init__(self) -> None:
        pass


class D(B, A):
    def __init__(self) -> None:
        pass
