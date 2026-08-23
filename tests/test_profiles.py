import unittest
from unittest.mock import patch

from rdp_tui.profiles import Profile, command_for, freerdp_client, load_profiles, save_profiles


class ProfileTests(unittest.TestCase):
    def test_save_load_and_command(self):
        with self.subTest("round trip"):
            from tempfile import TemporaryDirectory

            with TemporaryDirectory() as directory:
                from pathlib import Path

                path = Path(directory) / "profiles.json"
                profile = Profile("Work", "rdp.example.test", "ada", "EXAMPLE", audio=True)
                save_profiles([profile], path)
                self.assertEqual(load_profiles(path), [profile])

        self.assertEqual(command_for(profile, "xfreerdp3"), [
            "xfreerdp3", "/v:rdp.example.test", "/u:ada", "/d:EXAMPLE", "/f", "+clipboard", "/sound",
        ])

    @patch("rdp_tui.profiles.shutil.which")
    def test_prefers_freerdp3(self, which):
        which.side_effect = lambda executable: "/usr/bin/xfreerdp3" if executable == "xfreerdp3" else None
        self.assertEqual(freerdp_client(), "xfreerdp3")
