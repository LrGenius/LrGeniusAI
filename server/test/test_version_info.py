from src.version_info import BACKEND_VERSION, BACKEND_RELEASE_TAG, BACKEND_BUILD


def test_version_info_types():
    assert isinstance(BACKEND_VERSION, str)
    assert isinstance(BACKEND_RELEASE_TAG, str)
    assert isinstance(BACKEND_BUILD, int)


def test_version_info_structure():
    assert BACKEND_VERSION != ""
    assert BACKEND_BUILD >= 0
    # CI normally replaces this with v{Version}, locally it might be v0.0.0-dev
    assert BACKEND_RELEASE_TAG.startswith("v") or "-dev" in BACKEND_RELEASE_TAG
