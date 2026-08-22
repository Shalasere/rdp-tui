import unittest

from rdp_tui.profiles import Profile, command_for, load_profiles, save_profiles


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

        self.assertEqual(command_for(profile), [
            "xfreerdp", "/v:rdp.example.test", "/u:ada", "/d:EXAMPLE", "/f", "+clipboard", "/sound",
        ])
