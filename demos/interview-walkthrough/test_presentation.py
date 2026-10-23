"""Check the live deck's pacing and duplicated embedded data without a browser."""
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parent

class PresentationTests(unittest.TestCase):
    def test_live_deck_contract(self):
        slides = json.loads((ROOT / 'slides.json').read_text())
        html = (ROOT / 'slides.html').read_text()
        embedded = re.search(r'<script id="slide-data" type="application/json">(.*?)</script>', html, re.S)
        self.assertEqual(slides, json.loads(embedded.group(1)))
        end = 0
        for slide in slides:
            start, stop = map(int, slide['clock'].split('–'))
            self.assertEqual(start, end)
            self.assertGreater(stop, start)
            end = stop
            for field in ('what', 'how', 'why', 'screen', 'live', 'transition', 'question', 'source'):
                self.assertTrue(slide[field])
        self.assertEqual(end, 45)
        self.assertEqual(slides[-1]['clock'], '39–45')
        self.assertNotIn('DemoCpuDrop', json.dumps(slides))
        self.assertNotIn('demo_cpu_utilization_percent', json.dumps(slides))
        self.assertIn('data-src="animation/player.html"', html)
        self.assertNotRegex(html, r'(?<!data-)src="animation/player.html"')
        self.assertNotIn('autoplay', html)
        self.assertIn('Accept alert records intake', html)

if __name__ == '__main__':
    unittest.main()
