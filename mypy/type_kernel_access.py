"""Shared stale-kernel remedy for gated `type_kernel` seam fetches.

A stale `type_kernel` build predating a seam imports fine but lacks that
seam attribute, so the staleness surfaces only as a missing module
attribute on the first gated fetch through the seam (#36, #42, #98).
Every gated fetch must fail with this pointed remedy instead of a bare
AttributeError that reads as an INTERNAL ERROR.
"""

STALE_TYPE_KERNEL_REMEDY = (
    "rebuild the in-repo type_kernel extension and prepend its scratch directory "
    "to PYTHONPATH (see AGENTS.md, 'Type kernel build order')"
)


def _stale_kernel_remedy(err: AttributeError) -> RuntimeError:
    """Pointed rebuild remedy for a seam attribute a stale kernel lacks.

    Each gated fetch keeps its own typed `except AttributeError` so the
    module-attribute lookup stays precise; this helper is the single
    source of the remedy message.
    """
    return RuntimeError(
        "the type_kernel on sys.path is not the in-repo extension: "
        f"{err}. " + STALE_TYPE_KERNEL_REMEDY
    )
