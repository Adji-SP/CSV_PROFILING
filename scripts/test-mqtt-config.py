"""Pure configuration tests. Never touch the real .env or broker accounts."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("provision_mqtt", Path(__file__).with_name("provision-mqtt.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EnvironmentUpdateTests(unittest.TestCase):
    def test_keeps_other_settings_and_removes_duplicate_accounts(self):
        original = '# hello\r\nWIFI_PASS="unchanged"\r\n MQTT_USERNAME=old\r\nexport MQTT_USERNAME=older\n'
        result = module.update_env(original, {"MQTT_USERNAME": "console-backend", "MQTT_CERT_FILE": None})
        self.assertIn('WIFI_PASS="unchanged"', result)
        self.assertEqual(result.count("MQTT_USERNAME="), 1)
        self.assertIn('MQTT_USERNAME="console-backend"', result)
        self.assertNotIn("MQTT_CERT_FILE", result)

    def test_escapes_values_without_interpolating(self):
        result = module.update_env("", {"MQTT_PASSWORD": 'a"b\\c$HOME'})
        self.assertEqual(result, 'MQTT_PASSWORD="a\\"b\\\\c$HOME"\n')

    def test_rejects_multiline_instead_of_corrupting_env(self):
        for setting in ("DEVICE_MQTT_CA_PEM", "MQTT_PASSWORD"):
            with self.assertRaises(ValueError) as error:
                module.update_env(setting + '="private-value\nmore"\n', {setting: None})
            self.assertNotIn("private-value", str(error.exception))

    def test_quoted_single_line_with_comment(self):
        result = module.update_env('MQTT_PASSWORD="old" # comment\n', {"MQTT_PASSWORD": "new"})
        self.assertEqual(result, 'MQTT_PASSWORD="new"\n')


if __name__ == "__main__":
    unittest.main()
