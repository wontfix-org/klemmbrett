import pytest

import klemmbrett as kb
from klemmbrett import _registry, providers
from klemmbrett import ranking as rk


def ranking_of(name):
    return next(w for w in _registry.wire()["widgets"] if w["name"] == name)["ranking"]


def test_matchers_and_their_wire():
    assert rk.substring()._wire() == {"kind": "substring"}
    assert rk.words()._wire() == {"kind": "words"}
    assert rk.fuzzy(scheme="path", case="ignore", syntax=False)._wire() == {
        "kind": "fuzzy",
        "scheme": "path",
        "case": "ignore",
        "syntax": False,
    }
    with pytest.raises(ValueError, match="scheme"):
        rk.fuzzy(scheme="regex")
    with pytest.raises(ValueError, match="case"):
        rk.fuzzy(case="upper")


def test_rankers_add_up_and_scale():
    rank = rk.frecency(half_life="12h") + 2 * rk.learned(prefix=2) + rk.count(namespace="shared") * 0.5
    assert rank._wire() == [
        {"kind": "frecency", "weight": 1.0, "half_life": 43200.0},
        {"kind": "learned", "weight": 2.0, "half_life": 14 * 86400.0, "prefix": 2},
        {"kind": "count", "weight": 0.5, "namespace": "shared"},
    ]
    assert sum([rk.recent(), rk.count()]) == rk.recent() + rk.count()
    assert rk.pinned("a", boost=5)._wire() == [{"kind": "pinned", "weight": 1.0, "boost": 5.0, "keys": ["a"]}]


def test_bad_rankers_are_rejected():
    with pytest.raises(ValueError, match="duration"):
        rk.frecency(half_life="soon")
    with pytest.raises(ValueError, match="greater than zero"):
        rk.frecency(half_life=0)
    with pytest.raises(ValueError, match="prefix"):
        rk.learned(prefix=0)
    with pytest.raises(ValueError, match="namespace"):
        rk.recent(namespace=" ")
    with pytest.raises(TypeError, match="keys"):
        rk.pinned([1])
    with pytest.raises(TypeError, match="function"):
        rk.score(3)
    with pytest.raises(TypeError):
        rk.recent() * "2"


def test_python_rankers_collapse_into_one_term_and_score_in_python(capsys):
    rank = 2 * rk.score(lambda record: len(record.value)) + rk.pinned(lambda record: record.value == "a", boost=10)
    assert rank._wire() == [{"kind": "python", "weight": 1.0}]
    assert rank.scores([kb.Record("a"), kb.Record("bbb")]) == [12.0, 6.0]

    broken = rk.score(lambda record: 1 / 0) + rk.score(lambda record: "high")
    assert broken.scores([kb.Record("a"), kb.Record("b")]) == [0.0, 0.0]
    assert capsys.readouterr().err.count("ZeroDivisionError") == 1


def test_widgets_fetch_scores_with_their_records():
    kb.widget("w", providers.Static(["a", "bb"]), rank=rk.score(lambda record: len(record.value)))
    kb.widget("plain", providers.Static(["a"]), rank=rk.count())
    widgets = {w["name"]: w for w in _registry.wire()["widgets"]}
    assert widgets["w"]["source"]() == ([("a", "a", "a", None), ("bb", "bb", "bb", None)], [1.0, 2.0])
    assert widgets["plain"]["source"]() == [("a", "a", "a", None)]


def test_providers_supply_defaults_that_widgets_override():
    kb.widget("run", providers.Executables())
    kb.widget("run-plain", providers.Executables(), match=None, rank=None)
    kb.widget("run-usage", providers.Executables(), rank=rk.count(), prefer="usage")
    kb.widget("clips", providers.ClipboardHistory(path="/tmp/klemmbrett-test.sqlite"))
    kb.widget("static", providers.Static(["a"]))
    assert ranking_of("run") == {
        "match": rk.fuzzy()._wire(),
        "rank": (rk.frecency() + rk.learned())._wire(),
        "prefer": "blend",
    }
    assert ranking_of("run-plain") == {"match": {"kind": "substring"}, "rank": [], "prefer": "blend"}
    assert ranking_of("run-usage") == {"match": rk.fuzzy()._wire(), "rank": rk.count()._wire(), "prefer": "usage"}
    assert ranking_of("clips")["match"] == rk.fuzzy(scheme="history")._wire()
    assert ranking_of("static") == {"match": {"kind": "substring"}, "rank": [], "prefer": "blend"}


def test_views_inherit_and_merges_keep_only_agreed_defaults():
    windows = providers.WaylandWindows()
    assert providers.defaults(providers.View(windows, filter=lambda record: True)) == (rk.fuzzy(), rk.recent())
    assert providers.defaults(providers.Merge(providers.Executables(), providers.Executables())) == (
        rk.fuzzy(),
        rk.frecency() + rk.learned(),
    )
    assert providers.defaults(providers.Merge(windows, providers.Executables())) == (rk.fuzzy(), None)
    assert providers.defaults(providers.Merge(windows, providers.Static(["a"]))) == (None, None)

    class Bookmarks(kb.Provider):
        default_match = rk.words()

        def records(self):
            return []

    assert providers.defaults(Bookmarks()) == (rk.words(), None)
    assert providers.defaults(lambda: []) == (None, None)


def test_widget_arguments_are_checked():
    with pytest.raises(TypeError, match="match must be a matcher"):
        kb.widget("a", match="fuzzy")
    with pytest.raises(TypeError, match="rank must be rankers"):
        kb.widget("b", rank=rk.fuzzy())
    with pytest.raises(ValueError, match="prefer"):
        kb.widget("c", prefer="sometimes")
