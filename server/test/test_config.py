from src.config import _deep_merge_dict, get_culling_config, BASE_CULLING_CONFIG

def test_deep_merge_dict():
    base = {"a": 1, "b": {"c": 2, "d": 3}}
    override = {"b": {"c": 4, "e": 5}, "f": 6}
    merged = _deep_merge_dict(base, override)
    
    assert merged["a"] == 1
    assert merged["b"]["c"] == 4
    assert merged["b"]["d"] == 3
    assert merged["b"]["e"] == 5
    assert merged["f"] == 6

def test_get_culling_config_default():
    config = get_culling_config()
    assert config == BASE_CULLING_CONFIG
    
    config_explicit = get_culling_config("default")
    assert config_explicit == BASE_CULLING_CONFIG

def test_get_culling_config_portrait():
    config = get_culling_config("portrait")
    
    # Check that it merged overrides
    assert config["ranking"]["face_group_weight_technical"] == 0.34
    assert config["ranking"]["face_group_weight_face"] == 0.66
    
    # Check that base keys are still present
    assert "face_metrics" in config
    assert config["face_metrics"]["eye_patch_ratio"] == 0.08

def test_get_culling_config_invalid_preset():
    # Invalid presets should fallback to default
    config = get_culling_config("nonexistent_preset_999")
    assert config == BASE_CULLING_CONFIG
