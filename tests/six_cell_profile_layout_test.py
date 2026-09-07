import pathlib
import re
import unittest


REPOSITORY_ROOT = pathlib.Path(__file__).resolve().parents[1]


def function_body(relative_path: str, name: str) -> str:
    source = (REPOSITORY_ROOT / relative_path).read_text(encoding="utf-8")
    matched = re.search(
        rf"function {re.escape(name)}\([^)]*\) \{{(?P<body>.*?)\n\}}",
        source,
        re.DOTALL,
    )
    if matched is None:
        raise AssertionError(f"missing {name} in {relative_path}")
    return matched.group("body")


class SixCellProfileLayoutTest(unittest.TestCase):
    def test_qualify_uses_the_real_cargo_profile_directory(self) -> None:
        body = function_body("scripts/qjs/six-cell-qualify.qjs", "artifact_dir")
        self.assertIn('let leaf = profile;', body)
        self.assertIn('if (profile === "dev") { leaf = "debug"; }', body)
        self.assertNotIn('profile === "release-fast"', body)

    def test_packaging_uses_the_real_cargo_profile_directory(self) -> None:
        body = function_body(
            "scripts/qjs/package-six-cell-delivery.qjs", "artifact_leaf"
        )
        self.assertIn('if (profile === "dev") { return "debug"; }', body)
        self.assertIn('return profile;', body)
        self.assertNotIn('profile === "release-fast"', body)


if __name__ == "__main__":
    unittest.main()
