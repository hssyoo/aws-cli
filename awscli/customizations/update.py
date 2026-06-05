# Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License"). You
# may not use this file except in compliance with the License. A copy of
# the License is located at
#
#     http://aws.amazon.com/apache2.0/
#
# or in the "license" file accompanying this file. This file is
# distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF
# ANY KIND, either express or implied. See the License for the specific
# language governing permissions and limitations under the License.
"""``aws update`` — update the AWS CLI to the latest release in place.

Delegates to install.sh / install.ps1 to do the actual install. Source
distributions are unsupported.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from dataclasses import dataclass

from awscli.clidriver import (
    INSTALL_FILENAME,
    _get_distribution_source,
)
from awscli.compat import is_windows
from awscli.customizations.commands import BasicCommand


@dataclass
class InstallScriptConfig:
    """Inputs that vary across install modes. Extend with sensible defaults."""

    is_system: bool
    install_dir: str = None
    bin_dir: str = None


class UpdateCommand(BasicCommand):
    NAME = 'update'
    DESCRIPTION = (
        'Update the AWS CLI to the latest version, preserving the current '
        'installation directory. Only supported for installations created '
        'by the official installers or the curl|bash install script.'
    )
    SYNOPSIS = 'aws update'
    ARG_TABLE = []

    # TEMPORARY: dev CloudFront origin used for preview builds. Will be
    # pointed back at https://awscli.amazonaws.com before launch.
    DOWNLOAD_BASE_URL = 'https://d2j4ws18f0zfbf.cloudfront.net'
    WINDOWS_SCRIPT_URL = f'{DOWNLOAD_BASE_URL}/v2/install.ps1'
    UNIX_SCRIPT_URL = f'{DOWNLOAD_BASE_URL}/v2/install.sh'
    SUPPORTED_SOURCES = ('exe', 'script-exe', 'update-exe')
    UNIX_SYSTEM_INSTALL_DIR = '/usr/local/aws-cli'

    def _run_main(self, parsed_args, parsed_globals):
        source = _get_distribution_source()
        if source not in self.SUPPORTED_SOURCES:
            raise UpdateError(
                f"'aws update' does not support installations with "
                f"distribution_source={source!r}. Supported sources are: "
                f"{', '.join(self.SUPPORTED_SOURCES)}."
            )

        config = self._build_config()
        sys.stdout.write(f"Updating AWS CLI (source: {source})\n")

        with tempfile.TemporaryDirectory() as tmp:
            script_path = self._download_install_script(tmp)
            sys.stdout.write('Running install script...\n')
            self._run_install_script(script_path, config)
        # Do not add logic past this point: install script has rewritten
        # the running binary; the awscli package in memory is now stale.
        return 0

    def _build_config(self):
        if is_windows:
            return InstallScriptConfig(
                is_system=self._is_windows_system_install(),
            )
        install_dir, bin_dir = self._read_unix_install_paths()
        return InstallScriptConfig(
            is_system=(install_dir == self.UNIX_SYSTEM_INSTALL_DIR),
            install_dir=install_dir,
            bin_dir=bin_dir,
        )

    def _is_windows_system_install(self):
        program_files = os.environ.get('ProgramFiles')
        if not program_files:
            return False
        canonical = os.path.join(
            program_files, 'Amazon', 'AWSCLIV2', 'aws.exe'
        )
        return os.path.normcase(
            os.path.realpath(sys.executable)
        ) == os.path.normcase(canonical)

    def _read_unix_install_paths(self):
        import awscli  # local import to avoid a top-level cycle

        path = os.path.join(
            os.path.dirname(os.path.abspath(awscli.__file__)),
            'data',
            INSTALL_FILENAME,
        )
        try:
            with open(path) as f:
                install = json.load(f)
            return install['install_dir'], install['bin_dir']
        except (OSError, KeyError, ValueError):
            raise UpdateError(
                'install.json is missing or incomplete. This CLI install '
                'was either not produced by a supported installer or its '
                'install directory has been modified. Reinstall via the '
                'official installer or install script and try again.'
            )

    def _download_install_script(self, tmp_dir):
        url = self.WINDOWS_SCRIPT_URL if is_windows else self.UNIX_SCRIPT_URL
        ext = '.ps1' if is_windows else '.sh'
        dest = os.path.join(tmp_dir, f'install{ext}')
        self._download_with_retry(url, dest, retries=1)
        return dest

    def _download_with_retry(self, url, dest, retries):
        sys.stdout.write(f"Downloading {url}\n")
        for attempt in range(retries + 1):
            try:
                with (
                    urllib.request.urlopen(url, timeout=30) as resp,
                    open(dest, 'wb') as out,
                ):
                    shutil.copyfileobj(resp, out)
                return
            except (urllib.error.URLError, OSError) as exc:
                if attempt == retries:
                    raise UpdateError(f"failed to download {url}: {exc}")
                sys.stderr.write(f"download failed ({exc}); retrying...\n")

    def _run_install_script(self, script_path, config):
        env = os.environ.copy()
        env['AWS_CLI_DISTRIBUTION_SOURCE_OVERRIDE'] = 'update-exe'
        if is_windows:
            cmd = self._windows_install_cmd(script_path, config)
        else:
            cmd = self._unix_install_cmd(script_path, config)
            if not config.is_system:
                # install.sh appends "/aws-cli" to XDG_DATA_HOME.
                env['XDG_DATA_HOME'] = os.path.dirname(config.install_dir)
                env['XDG_BIN_HOME'] = config.bin_dir
        subprocess.run(cmd, env=env, check=True)

    def _unix_install_cmd(self, script_path, config):
        cmd = ['bash', script_path]
        if config.is_system:
            cmd.append('--system')
        return cmd

    def _windows_install_cmd(self, script_path, config):
        ps_exe = (
            shutil.which('pwsh')
            or shutil.which('powershell')
            or 'powershell.exe'
        )
        cmd = [ps_exe, '-NoProfile', '-File', script_path]
        if config.is_system:
            cmd.append('-System')
        return cmd


def register_update_command(event_handlers):
    event_handlers.register(
        'building-command-table.main', UpdateCommand.add_command
    )


class UpdateError(Exception):
    pass
