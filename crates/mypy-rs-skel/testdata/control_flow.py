def clamp(value: float, low: float, high: float) -> float:
    if value < low:
        return low
    if value > high:
        return high
    return value


def is_even(n: int) -> bool:
    return n % 2 == 0


def classify(count: int) -> str:
    if count < 0:
        return "negative"
    elif count == 0:
        return "zero"
    else:
        return "positive"


def max_of_two(a: int, b: int) -> int:
    if a > b:
        return a
    else:
        return b


result: float = clamp(5.0, 0.0, 3.0)
flag: bool = is_even(4)
label: str = classify(0)
winner: int = max_of_two(3, 9)
negated: bool = not flag
both: bool = is_even(2) and is_even(4)
