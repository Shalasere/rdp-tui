import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from rdp_tui.app import (
    filtered_profiles,
    fullscreen_wayland_sdl_window,
    preflight_profile,
    profile_status_lines,
    should_fallback_to_x11,
    status_text,
    tcp_rdp_reachable,
)
from rdp_tui.profile_io import export_rdp, import_profiles, merge_profiles
from rdp_tui.profiles import (
    Profile,
    command_for,
    freerdp_client,
    load_profiles,
    local_display_resolution,
    local_display_settings,
    resolved_host,
    save_profiles,
    validate_profile,
)
from rdp_tui.secrets import _delete_file_password, _file_password, _save_file_password, resolved_backend


class ProfileTests(unittest.TestCase):
    def test_save_load_and_command(self):
        with self.subTest("round trip"):
            from tempfile import TemporaryDirectory

            with TemporaryDirectory() as directory:
                from pathlib import Path

                path = Path(directory) / "profiles.json"
                profile = Profile("Work", "rdp.example.test", user="ada", domain="EXAMPLE", audio=True, renderer="x11")
                save_profiles([profile], path)
                self.assertEqual(load_profiles(path), [profile])

        self.assertEqual(
            command_for(profile, "xfreerdp3"),
            [
                "xfreerdp3",
                "/v:rdp.example.test",
                "/u:ada",
                "/d:EXAMPLE",
                "/f",
                "+clipboard",
                "/sound",
            ],
        )

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

    def test_wayland_sdl_starts_windowed_at_detected_size(self):
        profile = Profile("Display", "10.0.0.41", renderer="wayland_sdl", admin_session=True)
        command = command_for(profile, "sdl-freerdp3", detected_resolution="1920x1080")
        self.assertIn("/size:1920x1080", command)
        self.assertNotIn("/f", command)
        self.assertIn("/admin", command)
        self.assertIn("-grab-keyboard", command)
        self.assertIn("-grab-mouse", command)

    def test_builds_gateway_without_exposing_password(self):
        profile = Profile(
            "Gateway",
            "internal.example",
            gateway_host="gateway.example:8443",
            gateway_user="ada",
            gateway_domain="EXAMPLE",
        )
        command = command_for(profile)
        self.assertIn("/gateway:g:gateway.example:8443,u:ada,d:EXAMPLE", command)
        self.assertFalse(any("password" in option.lower() for option in command))
        self.assertEqual(validate_profile(profile), [])

    def test_validates_ssh_tunnel_config_host(self):
        self.assertEqual(validate_profile(Profile("Tunnel", "server", ssh_tunnel_host="jump-host")), [])
        self.assertIn(
            "SSH tunnel host cannot contain whitespace",
            validate_profile(Profile("Tunnel", "server", ssh_tunnel_host="bad host")),
        )

    def test_migrates_null_storage_fields(self):
        profile = Profile.from_dict({"name": "Legacy", "host": "10.0.0.41", "id": None, "password_backend": None})
        self.assertIsInstance(profile.id, str)
        self.assertEqual(profile.password_backend, "automatic")

    def test_rejects_invalid_host_port_and_options(self):
        profile = Profile("Bad", "rdp.example.test:99999", extra_options='"unterminated')
        errors = validate_profile(profile)
        self.assertIn("Port must be between 1 and 65535", errors)
        self.assertTrue(any(error.startswith("Extra options are invalid") for error in errors))
        self.assertIn("RDP renderer is invalid", validate_profile(Profile("Bad", "10.0.0.41", renderer="bad")))

    def test_builds_advanced_rdp_options(self):
        profile = Profile(
            "Advanced",
            "10.0.0.41",
            resolution="1920x1080",
            dynamic_resolution=True,
            smart_sizing=True,
            scale=140,
            microphone=True,
            auto_reconnect=True,
            network_type="lan",
            color_depth=32,
            certificate_policy="tofu",
        )
        command = command_for(profile)
        for option in (
            "/size:1920x1080",
            "+dynamic-resolution",
            "/smart-sizing",
            "/scale:140",
            "/microphone",
            "+auto-reconnect",
            "/network:lan",
            "/bpp:32",
            "/cert:tofu",
        ):
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

    @patch("rdp_tui.app.freerdp_client", return_value="xfreerdp3")
    def test_status_reports_ready_client(self, _client):
        self.assertEqual(status_text(), "Status: Ready — xfreerdp3 detected.")

    @patch("rdp_tui.app.local_display_settings", return_value=("1920x1080", 150))
    @patch("rdp_tui.app.password_for", return_value="secret")
    @patch("rdp_tui.app.resolved_backend", return_value="encrypted_file")
    @patch("rdp_tui.app.freerdp_client", return_value="sdl-freerdp3")
    def test_profile_status_shows_effective_connection_details(self, _client, _backend, _password, _display):
        profile = Profile(
            "LAN", "10.0.0.41", user="apple", renderer="wayland_sdl", admin_session=True, smart_sizing=True
        )
        lines = profile_status_lines(
            profile, {"profile_id": profile.id, "exit_code": 0, "elapsed_seconds": 1.2, "finished_at": "today"}
        )
        report = "\n".join(lines)
        self.assertIn("sdl-freerdp3", report)
        self.assertIn("1920x1080", report)
        self.assertIn("console (/admin)", report)
        self.assertIn("saved (encrypted file)", report)
        self.assertIn("Last session: completed", report)

    @patch("rdp_tui.app.subprocess.run")
    def test_fullscreens_mapped_sdl_window_without_client_fullscreen(self, run):
        from types import SimpleNamespace

        run.side_effect = [
            SimpleNamespace(stdout='[{"pid": 42, "address": "0xabc"}]'),
            SimpleNamespace(returncode=0, stderr=""),
        ]
        self.assertTrue(fullscreen_wayland_sdl_window(42))
        expression = run.call_args_list[1].args[0][2]
        self.assertIn("internal = 2, client = 0", expression)
        self.assertIn("address:0xabc", expression)

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

    def test_import_remmina_and_export_rdp_without_password(self):
        with TemporaryDirectory() as directory:
            remmina = Path(directory) / "office.remmina"
            remmina.write_text(
                "[remmina]\nprotocol=RDP\nname=Office\nserver=rdp.example\nusername=EXAMPLE\\ada\nresolution_width=1920\nresolution_height=1080\n"
            )
            profile = import_profiles(remmina)[0]
            self.assertEqual(
                (profile.name, profile.user, profile.domain, profile.resolution),
                ("Office", "ada", "EXAMPLE", "1920x1080"),
            )
            exported = Path(directory) / "office.rdp"
            export_rdp(profile, exported)
            contents = exported.read_text()
            self.assertIn("full address:s:rdp.example", contents)
            self.assertNotIn("password", contents.lower())
            imported = import_profiles(exported)[0]
            self.assertEqual(imported.host, "rdp.example")

    def test_imports_remmina_directory_and_advanced_settings(self):
        with TemporaryDirectory() as directory:
            source = Path(directory)
            (source / "one.remmina").write_text(
                "[remmina]\n"
                "protocol=RDP\nname=Desktop\nserver=desktop.example\nusername=EXAMPLE\\ada\n"
                "viewmode=1\nresolution_width=0\nresolution_height=0\nsound=local\nscale=1\n"
                "disableautoreconnect=0\nnetwork=autodetect\ncolordepth=99\nconsole=1\n"
                "gateway_server=gateway.example\ngateway_username=gate-user\ngateway_domain=EXAMPLE\n"
            )
            (source / "two.remmina").write_text(
                "[remmina]\nprotocol=RDP\nname=Laptop\nserver=laptop.example\n"
                "viewmode=4\nresolution_width=1920\nresolution_height=1080\nscale=2\n"
                "force_multimon=1\nspan=0\ndisableautoreconnect=1\nnetwork=lan\ncolordepth=24\n"
            )

            desktop, laptop = import_profiles(source)
            self.assertEqual(len(import_profiles(source)), 2)
            self.assertEqual(desktop.resolution, "")
            self.assertFalse(desktop.fullscreen)
            self.assertTrue(desktop.audio)
            self.assertTrue(desktop.smart_sizing)
            self.assertTrue(desktop.auto_reconnect)
            self.assertEqual(desktop.network_type, "auto")
            self.assertEqual(desktop.color_depth, 0)
            self.assertTrue(desktop.admin_session)
            self.assertEqual(
                (desktop.gateway_host, desktop.gateway_user, desktop.gateway_domain),
                ("gateway.example", "gate-user", "EXAMPLE"),
            )
            self.assertEqual(laptop.resolution, "1920x1080")
            self.assertTrue(laptop.fullscreen)
            self.assertFalse(laptop.dynamic_resolution)
            self.assertTrue(laptop.multimon)
            self.assertFalse(laptop.auto_reconnect)
            self.assertEqual(laptop.network_type, "lan")
            self.assertEqual(laptop.color_depth, 24)

    def test_merge_profiles_preserves_distinct_ids(self):
        current = [Profile("Current", "one", id="same")]
        incoming = [Profile("Imported", "two", id="same")]
        merged = merge_profiles(current, incoming)
        self.assertEqual(len({profile.id for profile in merged}), 2)

    def test_merge_profiles_skips_unchanged_imports(self):
        current = [Profile("Current", "one", id="existing")]
        incoming = [Profile("Current", "one", id="new")]
        self.assertEqual(merge_profiles(current, incoming), current)

    def test_filters_profiles_by_connection_fields(self):
        profiles = [Profile("Office", "rdp.example", user="ada", domain="EXAMPLE"), Profile("LAN", "10.0.0.41")]
        self.assertEqual(filtered_profiles(profiles, "example ada"), [profiles[0]])
        self.assertEqual(filtered_profiles(profiles, "10.0"), [profiles[1]])

    def test_only_falls_back_when_sdl_never_maps(self):
        self.assertTrue(should_fallback_to_x11("wayland_sdl", 1, False))
        self.assertFalse(should_fallback_to_x11("wayland_sdl", 1, True))
        self.assertFalse(should_fallback_to_x11("x11", 1, False))

    @patch("rdp_tui.app.socket.getaddrinfo", return_value=[(2, 1, 6, "", ("10.0.0.41", 3389))])
    @patch("rdp_tui.app.socket.socket")
    def test_tcp_preflight_reports_reachable_target(self, socket_factory, _addresses):
        socket_factory.return_value.__enter__.return_value.connect_ex.return_value = 0
        reachable, detail = tcp_rdp_reachable("10.0.0.41")
        self.assertTrue(reachable)
        self.assertIn("10.0.0.41:3389", detail)

    @patch("rdp_tui.app.tcp_rdp_reachable", return_value=(False, "connection refused"))
    @patch("rdp_tui.app.freerdp_client", return_value="xfreerdp3")
    def test_preflight_blocks_unreachable_target(self, _client, _network):
        issues = preflight_profile(Profile("LAN", "10.0.0.41"))
        self.assertEqual(issues, ["RDP network check failed: connection refused"])

    @patch("rdp_tui.secrets.keyring_available", return_value=False)
    def test_automatic_storage_falls_back_without_keyring(self, _available):
        self.assertEqual(resolved_backend("automatic"), "encrypted_file")
