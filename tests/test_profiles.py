import unittest
from unittest.mock import patch

from rdp_tui.profiles import Profile, command_for, freerdp_client, load_profiles, resolved_host, save_profiles, validate_profile
from rdp_tui.app import status_text
from rdp_tui.secrets import _delete_file_password, _file_password, _save_file_password, resolved_backend


class ProfileTests(unittest.TestCase):
    def test_save_load_and_command(self):
        with self.subTest("round trip"):
            from tempfile import TemporaryDirectory

            with TemporaryDirectory() as directory:
                from pathlib import Path

                path = Path(directory) / "profiles.json"
                profile = Profile("Work", "rdp.example.test", user="ada", domain="EXAMPLE", audio=True)
                save_profiles([profile], path)
                self.assertEqual(load_profiles(path), [profile])

        self.assertEqual(command_for(profile, "xfreerdp3"), [
            "xfreerdp3", "/v:rdp.example.test", "/u:ada", "/d:EXAMPLE", "/f", "+clipboard", "/sound",
        ])

    def test_empty_domain_is_explicit(self):
        command = command_for(Profile("LAN", "10.0.0.41"))
        self.assertIn("/d:", command)
        self.assertIn("/auth-pkg-list:!kerberos", command)

    @patch("rdp_tui.profiles.socket.gethostbyname", return_value="10.0.0.41")
    def test_resolves_mdns_to_ipv4(self, _lookup):
        self.assertEqual(resolved_host("compono.local"), "10.0.0.41")

    def test_migrates_null_storage_fields(self):
        profile = Profile.from_dict({"name": "Legacy", "host": "10.0.0.41", "id": None, "password_backend": None})
        self.assertIsInstance(profile.id, str)
        self.assertEqual(profile.password_backend, "automatic")

    def test_rejects_invalid_host_port_and_options(self):
        profile = Profile("Bad", "rdp.example.test:99999", extra_options='"unterminated')
        errors = validate_profile(profile)
        self.assertIn("Port must be between 1 and 65535", errors)
        self.assertTrue(any(error.startswith("Extra options are invalid") for error in errors))

    @patch("rdp_tui.profiles.shutil.which")
    def test_prefers_freerdp3(self, which):
        which.side_effect = lambda executable: "/usr/bin/xfreerdp3" if executable == "xfreerdp3" else None
        self.assertEqual(freerdp_client(), "xfreerdp3")

    @patch("rdp_tui.app.freerdp_client", return_value="xfreerdp3")
    def test_status_reports_ready_client(self, _client):
        self.assertEqual(status_text(), "Status: Ready — xfreerdp3 detected.")

    def test_encrypted_file_password_store(self):
        from pathlib import Path
        from tempfile import TemporaryDirectory

        with TemporaryDirectory() as directory:
            secrets_path = Path(directory) / "secrets.json"
            key_path = Path(directory) / ".password-key"
            _save_file_password("profile-id", "correct horse battery staple", secrets_path, key_path)
            self.assertEqual(_file_password("profile-id", secrets_path, key_path), "correct horse battery staple")
            self.assertNotIn("correct horse battery staple", secrets_path.read_text())
            _delete_file_password("profile-id", secrets_path)
            self.assertIsNone(_file_password("profile-id", secrets_path, key_path))

    @patch("rdp_tui.secrets.keyring_available", return_value=False)
    def test_automatic_storage_falls_back_without_keyring(self, _available):
        self.assertEqual(resolved_backend("automatic"), "encrypted_file")
