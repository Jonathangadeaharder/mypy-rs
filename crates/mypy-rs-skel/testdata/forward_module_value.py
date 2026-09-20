def reader() -> int:
    return value


def caller() -> int:
    return helper()


value = 1


def helper() -> int:
    return value
