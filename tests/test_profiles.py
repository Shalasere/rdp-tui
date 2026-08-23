import unittest
from unittest.mock import patch

from rdp_tui.profiles import (Profile, command_for, freerdp_client, load_profiles, local_display_resolution, local_display_settings,
                              resolved_host, save_profiles, validate_profile)
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
        self.assertIn("/auth-pkg-list:none,ntlm", command)

    @patch("rdp_tui.profiles.socket.gethostbyname", return_value="10.0.0.41")
    def test_resolves_mdns_to_ipv4(self, _lookup):
        self.assertEqual(resolved_host("compono.local"), "10.0.0.41")

    @patch("rdp_tui.profiles.subprocess.run")
    @patch("rdp_tui.profiles.shutil.which", return_value="/usr/bin/hyprctl")
    def test_detects_focused_hyprland_resolution(self, _which, run):
        run.return_value.stdout = '[{"focused": false, "width": 2560, "height": 1440}, {"focused": true, "width": 1920, "height": 1080, "scale": 1.5}]'
        self.assertEqual(local_display_resolution(), "1920x1080")
        self.assertEqual(local_display_settings(), ("1920x1080", 150))

    def test_uses_detected_resolution_when_profile_has_none(self):
        command = command_for(Profile("Display", "10.0.0.41"), detected_resolution="1920x1080")
        self.assertIn("/size:1920x1080", command)
        self.assertNotIn("/scale-desktop:150", command)
        self.assertNotIn("/smart-sizing:1280x720", command)

    def test_migrates_null_storage_fields(self):
        profile = Profile.from_dict({"name": "Legacy", "host": "10.0.0.41", "id": None, "password_backend": None})
        self.assertIsInstance(profile.id, str)
        self.assertEqual(profile.password_backend, "automatic")

    def test_rejects_invalid_host_port_and_options(self):
        profile = Profile("Bad", "rdp.example.test:99999", extra_options='"unterminated')
        errors = validate_profile(profile)
        self.assertIn("Port must be between 1 and 65535", errors)
        self.assertTrue(any(error.startswith("Extra options are invalid") for error in errors))

    def test_builds_advanced_rdp_options(self):
        profile = Profile(
            "Advanced", "10.0.0.41", resolution="1920x1080", dynamic_resolution=True,
            smart_sizing=True, scale=140, microphone=True, auto_reconnect=True,
            network_type="lan", color_depth=32, certificate_policy="tofu",
        )
        command = command_for(profile)
        for option in ("/size:1920x1080", "+dynamic-resolution", "/smart-sizing", "/scale:140", "/microphone",
                       "+auto-reconnect", "/network:lan", "/bpp:32", "/cert:tofu"):
            self.assertIn(option, command)

    def test_rejects_conflicting_advanced_display_options(self):
        profile = Profile("Conflict", "10.0.0.41", dynamic_resolution=True, multimon=True)
        self.assertIn("Dynamic resolution cannot be used with multi-monitor", validate_profile(profile))

        profile = Profile("Conflict", "10.0.0.41", dynamic_resolution=True, smart_sizing=True)
        self.assertIn("Dynamic resolution cannot be used with smart sizing", validate_profile(profile))

    def test_builds_folder_and_multimon_options(self):
        from tempfile import TemporaryDirectory

        with TemporaryDirectory() as folder:
            profile = Profile("Shares", "10.0.0.41", shared_folder=folder, multimon=True)
            command = command_for(profile)
            self.assertIn(f"/drive:rdp-tui,{folder}", command)
            self.assertIn("/multimon", command)
            self.assertEqual(validate_profile(profile), [])

    @patch("rdp_tui.profiles.shutil.which")
    def test_prefers_freerdp3(self, which):
        which.side_effect = lambda executable: "/usr/bin/xfreerdp3" if executable == "xfreerdp3" else None
        self.assertEqual(freerdp_client(), "xfreerdp3")

    @patch.dict("rdp_tui.profiles.os.environ", {"WAYLAND_DISPLAY": "wayland-1"}, clear=True)
    @patch("rdp_tui.profiles.shutil.which", return_value="/usr/bin/wlfreerdp3")
    def test_prefers_wayland_freerdp_on_wayland(self, _which):
        self.assertEqual(freerdp_client(), "wlfreerdp3")

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
