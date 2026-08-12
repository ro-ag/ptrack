"""Focused regression tests for Help Center navigation boundaries."""

import unittest

from tools import help_check


class ExternalURLTests(unittest.TestCase):
    def test_accepts_project_namespaces(self) -> None:
        self.assertTrue(help_check.allowed_external_url("https://github.com/ro-ag/ptrack"))
        self.assertTrue(help_check.allowed_external_url("https://github.com/ro-ag/ptrack/releases"))
        self.assertTrue(help_check.allowed_external_url("https://ro-ag.github.io/ptrack/help/"))

    def test_rejects_shared_host_and_normalization_escapes(self) -> None:
        rejected = (
            "https://github.com/attacker/ptrack/releases",
            "https://github.com/ro-ag/ptrack/../../attacker/releases",
            "https://ro-ag.github.io/ptrack/help/../../outside/",
            "https://ro-ag.github.io/ptrack/help%2f../../outside/",
            "https://ro-ag.github.io:443/ptrack/help/",
            "https://user@github.com/ro-ag/ptrack",
        )
        for target in rejected:
            with self.subTest(target=target):
                self.assertFalse(help_check.allowed_external_url(target))


class RouteContainmentTests(unittest.TestCase):
    def test_checked_routes_resolve_inside_help(self) -> None:
        resolved = help_check.route_target(help_check.HELP / "index.html", "start-here/")
        self.assertIsNotNone(resolved)
        destination, fragment = resolved
        self.assertEqual(destination, help_check.HELP / "start-here" / "index.html")
        self.assertEqual(fragment, "")

    def test_route_target_rejects_help_escape(self) -> None:
        with self.assertRaisesRegex(ValueError, "escapes docs/help"):
            help_check.route_target(help_check.HELP / "index.html", "../../frontend/")

    def test_route_target_rejects_encoded_help_escape(self) -> None:
        with self.assertRaisesRegex(ValueError, "escapes docs/help"):
            help_check.route_target(help_check.HELP / "index.html", "%2e%2e/%2e%2e/frontend/")


class DocumentAssetTests(unittest.TestCase):
    def test_parser_collects_link_assets(self) -> None:
        parser = help_check.HelpHTMLParser(help_check.HELP / "index.html")
        parser.feed('<link rel="stylesheet" href="assets/style.css"><link rel="icon" href="assets/favicon.svg">')
        self.assertIn(("href", "assets/style.css"), parser.links)
        self.assertIn(("href", "assets/favicon.svg"), parser.links)


class HomeVisualContractTests(unittest.TestCase):
    def test_homepage_has_theme_aware_product_preview(self) -> None:
        parser = help_check.HelpHTMLParser(help_check.HELP / "index.html")
        parser.feed((help_check.HELP / "index.html").read_text(encoding="utf-8"))
        sources = {image.get("src") for image in parser.images}
        self.assertIn("assets/screenshots/board-dark.png", sources)
        self.assertIn("assets/screenshots/board-light.png", sources)

    def test_homepage_cards_are_block_level(self) -> None:
        css = (help_check.HELP / "assets" / "style.css").read_text(encoding="utf-8")
        card_rule = css.split(".card {", 1)[1].split("}", 1)[0]
        self.assertIn("display: block;", card_rule)


if __name__ == "__main__":
    unittest.main()
