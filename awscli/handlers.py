# Copyright 2013 Amazon.com, Inc. or its affiliates. All Rights Reserved.
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
"""Builtin CLI extensions.

This is a collection of built in CLI extensions that can be automatically
registered with the event system.

"""

# ---- Eager imports: plugins that must register at startup ----
from awscli.alias import register_alias_commands
from awscli.argprocess import ParamShorthandParser
from awscli.clidriver import no_pager_handler
from awscli.customizations import datapipeline
from awscli.customizations.addexamples import add_examples
from awscli.customizations.argrename import register_arg_renames
from awscli.customizations.assumerole import register_assume_role_provider
from awscli.customizations.awslambda import register_lambda_create_function
from awscli.customizations.binaryformat import add_binary_formatter
from awscli.customizations.cliinput import register_cli_input_args
from awscli.customizations.cloudfront import register as register_cloudfront
from awscli.customizations.cloudsearch import initialize as cloudsearch_init
from awscli.customizations.cloudsearchdomain import register_cloudsearchdomain
from awscli.customizations.configservice.putconfigurationrecorder import (
    register_modify_put_configuration_recorder,
)
from awscli.customizations.ec2.addcount import register_count_events
from awscli.customizations.ec2.bundleinstance import register_bundleinstance
from awscli.customizations.ec2.decryptpassword import ec2_add_priv_launch_key
from awscli.customizations.ec2.paginate import register_ec2_page_size_injector
from awscli.customizations.ec2.protocolarg import register_protocol_args
from awscli.customizations.ec2.runinstances import register_runinstances
from awscli.customizations.ec2.secgroupsimplify import register_secgroup
from awscli.customizations.dynamodb.paginatorfix import (
    register_dynamodb_paginator_fix,
)
from awscli.customizations.generatecliskeleton import (
    register_generate_cli_skeleton,
)
from awscli.customizations.globalargs import register_parse_global_args
from awscli.customizations.history import register_history_mode
from awscli.customizations.iamvirtmfa import IAMVMFAWrapper
from awscli.customizations.iot import (
    register_create_keys_and_cert_arguments,
    register_create_keys_from_csr_arguments,
)
from awscli.customizations.iot_data import register_custom_endpoint_note
from awscli.customizations.kinesis import (
    register_kinesis_list_streams_pagination_backcompat,
)
from awscli.customizations.kms import register_fix_kms_create_grant_docs
from awscli.customizations.paginate import register_pagination
from awscli.customizations.putmetricdata import register_put_metric_data
from awscli.customizations.quicksight import (
    register_quicksight_asset_bundle_customizations,
)
from awscli.customizations.rds import (
    register_add_generate_db_auth_token,
    register_rds_modify_split,
)
from awscli.customizations.rekognition import (
    register_rekognition_detect_labels,
)
from awscli.customizations.removals import register_removals
from awscli.customizations.route53 import register_create_hosted_zone_doc_fix
from awscli.customizations.s3errormsg import register_s3_error_msg
from awscli.customizations.s3events import (
    register_document_expires_string,
    register_event_stream_arg,
)
from awscli.customizations.sessendemail import register_ses_send_email
from awscli.customizations.sso import register_sso_commands
from awscli.customizations.streamingoutputarg import add_streaming_output_arg
from awscli.customizations.timestampformat import register_timestamp_format
from awscli.customizations.toplevelbool import register_bool_params
from awscli.customizations.translate import (
    register_translate_import_terminology,
)
from awscli.customizations.waiters import register_add_waiters
from awscli.customizations.wizard.commands import register_wizard_commands
from awscli.paramfile import register_uri_param_handler

# ---- Lazy imports: deferred until the command/service is actually used ----
from awscli.lazy import LazyCommand, lazy_callback


def _add_lazy_s3(command_table, session, **kwargs):
    from awscli.customizations.utils import rename_command

    rename_command(command_table, 's3', 's3api')
    command_table['s3'] = LazyCommand(
        's3', session, 'awscli.customizations.s3.s3', 'S3'
    )


def _add_lazy_configure(command_table, session, **kwargs):
    command_table['configure'] = LazyCommand(
        'configure',
        session,
        'awscli.customizations.configure.configure',
        'ConfigureCommand',
    )


def _add_lazy_ddb(command_table, session, **kwargs):
    command_table['ddb'] = LazyCommand(
        'ddb', session, 'awscli.customizations.dynamodb.ddb', 'DDB'
    )


def _add_lazy_history(command_table, session, **kwargs):
    command_table['history'] = LazyCommand(
        'history',
        session,
        'awscli.customizations.history',
        'HistoryCommand',
    )


def _add_lazy_login(command_table, session, **kwargs):
    command_table['login'] = LazyCommand(
        'login',
        session,
        'awscli.customizations.login.login',
        'LoginCommand',
    )


def _add_lazy_logout(command_table, session, **kwargs):
    command_table['logout'] = LazyCommand(
        'logout',
        session,
        'awscli.customizations.login.logout',
        'LogoutCommand',
    )


def _add_lazy_cli_dev(command_table, session, **kwargs):
    command_table['cli-dev'] = LazyCommand(
        'cli-dev',
        session,
        'awscli.customizations.devcommands',
        'CLIDevCommand',
    )


def _rename_config(command_table, session, **kwargs):
    from awscli.customizations.utils import rename_command

    rename_command(command_table, 'config', 'configservice')


def _rename_codedeploy(command_table, session, **kwargs):
    from awscli.customizations.utils import rename_command

    rename_command(command_table, 'codedeploy', 'deploy')


def awscli_initialize(event_handlers):
    # ---- Eager: session/global events that fire every invocation ----
    event_handlers.register('session-initialized', register_uri_param_handler)
    event_handlers.register('session-initialized', add_binary_formatter)
    event_handlers.register('session-initialized', no_pager_handler)
    param_shorthand = ParamShorthandParser()
    event_handlers.register('process-cli-arg', param_shorthand)
    register_s3_error_msg(event_handlers)
    event_handlers.register('doc-examples.*.*', add_examples)
    register_cli_input_args(event_handlers)
    event_handlers.register(
        'building-argument-table.*', add_streaming_output_arg
    )
    register_count_events(event_handlers)
    event_handlers.register(
        'building-argument-table.ec2.get-password-data',
        ec2_add_priv_launch_key,
    )
    register_parse_global_args(event_handlers)
    register_pagination(event_handlers)
    register_secgroup(event_handlers)
    register_bundleinstance(event_handlers)
    register_runinstances(event_handlers)
    register_removals(event_handlers)
    register_rds_modify_split(event_handlers)
    register_rekognition_detect_labels(event_handlers)
    register_add_generate_db_auth_token(event_handlers)
    register_put_metric_data(event_handlers)
    register_ses_send_email(event_handlers)
    IAMVMFAWrapper(event_handlers)
    register_arg_renames(event_handlers)
    register_bool_params(event_handlers)
    register_protocol_args(event_handlers)
    cloudsearch_init(event_handlers)
    register_cloudsearchdomain(event_handlers)
    register_generate_cli_skeleton(event_handlers)
    register_assume_role_provider(event_handlers)
    register_add_waiters(event_handlers)
    register_timestamp_format(event_handlers)
    register_lambda_create_function(event_handlers)
    register_fix_kms_create_grant_docs(event_handlers)
    register_create_hosted_zone_doc_fix(event_handlers)
    register_modify_put_configuration_recorder(event_handlers)
    register_custom_endpoint_note(event_handlers)
    event_handlers.register(
        'building-argument-table.iot.create-keys-and-certificate',
        register_create_keys_and_cert_arguments,
    )
    event_handlers.register(
        'building-argument-table.iot.create-certificate-from-csr',
        register_create_keys_from_csr_arguments,
    )
    register_cloudfront(event_handlers)
    register_ec2_page_size_injector(event_handlers)
    register_translate_import_terminology(event_handlers)
    register_history_mode(event_handlers)
    register_event_stream_arg(event_handlers)
    register_document_expires_string(event_handlers)
    register_sso_commands(event_handlers)
    register_dynamodb_paginator_fix(event_handlers)
    register_alias_commands(event_handlers)
    register_kinesis_list_streams_pagination_backcompat(event_handlers)
    register_quicksight_asset_bundle_customizations(event_handlers)
    register_wizard_commands(event_handlers)
    datapipeline.register_customizations(event_handlers)

    # ---- TYPE A: LazyCommand for building-command-table.main ----
    # These add entries to the main command table but defer the module
    # import until the command is actually invoked.
    event_handlers.register('building-command-table.main', _add_lazy_s3)
    event_handlers.register(
        'building-command-table.s3_sync',
        lazy_callback(
            'awscli.customizations.s3.syncstrategy.register',
            'register_sync_strategies',
        ),
    )
    event_handlers.register('building-command-table.main', _add_lazy_configure)
    event_handlers.register('building-command-table.main', _add_lazy_ddb)
    event_handlers.register('building-command-table.main', _add_lazy_history)
    event_handlers.register('building-command-table.main', _add_lazy_login)
    event_handlers.register('building-command-table.main', _add_lazy_logout)
    event_handlers.register('building-command-table.main', _add_lazy_cli_dev)
    event_handlers.register('building-command-table.main', _rename_config)
    event_handlers.register('building-command-table.main', _rename_codedeploy)

    # ---- TYPE B: lazy_callback for service-specific command tables ----
    # These only fire when the specific service is invoked.

    # cloudformation (~28ms)
    event_handlers.register(
        'building-command-table.cloudformation',
        lazy_callback(
            'awscli.customizations.cloudformation', 'inject_commands'
        ),
    )

    # emr (~13ms)
    event_handlers.register(
        'building-command-table.emr',
        lazy_callback('awscli.customizations.emr.emr', 'register_commands'),
    )
    event_handlers.register(
        'building-argument-table.emr.add-tags',
        lazy_callback(
            'awscli.customizations.emr.addtags', 'modify_tags_argument'
        ),
    )
    event_handlers.register(
        'building-argument-table.emr.list-clusters',
        lazy_callback(
            'awscli.customizations.emr.listclusters',
            'modify_list_clusters_argument',
        ),
    )
    event_handlers.register(
        'before-building-argument-table-parser.emr.*',
        lazy_callback(
            'awscli.customizations.emr.command',
            'override_args_required_option',
        ),
    )

    # ecs (~8ms)
    event_handlers.register(
        'building-command-table.ecs',
        lazy_callback('awscli.customizations.ecs', 'inject_commands'),
    )
    # ecs monitor_mutating_gateway_service — uses instance methods,
    # defer the import but register eagerly when the module loads
    def _lazy_register_ecs_monitor(**kwargs):
        pass  # placeholder, see below

    def _register_ecs_monitor_on_first_ecs_event(
        command_table, session, **kwargs
    ):
        from awscli.customizations.ecs.monitormutatinggatewayservice import (
            register_monitor_mutating_gateway_service,
        )

        register_monitor_mutating_gateway_service(event_handlers)

    event_handlers.register(
        'building-command-table.ecs',
        _register_ecs_monitor_on_first_ecs_event,
    )

    # eks (~3ms)
    event_handlers.register(
        'building-command-table.eks',
        lazy_callback('awscli.customizations.eks', 'inject_commands'),
    )

    # emrcontainers (~4ms)
    event_handlers.register(
        'building-command-table.emr-containers',
        lazy_callback(
            'awscli.customizations.emrcontainers', 'inject_commands'
        ),
    )

    # lightsail (~1ms)
    event_handlers.register(
        'building-command-table.lightsail',
        lazy_callback('awscli.customizations.lightsail', 'inject_commands'),
    )

    # cloudtrail (~2ms)
    event_handlers.register(
        'building-command-table.cloudtrail',
        lazy_callback('awscli.customizations.cloudtrail', 'inject_commands'),
    )

    # ecr (~0.5ms)
    event_handlers.register(
        'building-command-table.ecr',
        lazy_callback('awscli.customizations.ecr', '_inject_commands'),
    )

    # ecr-public (~0.5ms)
    event_handlers.register(
        'building-command-table.ecr-public',
        lazy_callback('awscli.customizations.ecr_public', '_inject_commands'),
    )

    # configservice subscribe/getstatus (~3ms combined)
    event_handlers.register(
        'building-command-table.configservice',
        lazy_callback(
            'awscli.customizations.configservice.subscribe', 'add_subscribe'
        ),
    )
    event_handlers.register(
        'building-command-table.configservice',
        lazy_callback(
            'awscli.customizations.configservice.getstatus', 'add_get_status'
        ),
    )

    # codeartifact (~1ms)
    event_handlers.register(
        'building-command-table.codeartifact',
        lazy_callback(
            'awscli.customizations.codeartifact', 'inject_commands'
        ),
    )

    # codecommit (~1ms)
    event_handlers.register(
        'building-command-table.codecommit',
        lazy_callback('awscli.customizations.codecommit', 'inject_commands'),
    )

    # codedeploy — service-specific events (rename handled above in TYPE A)
    event_handlers.register(
        'building-command-table.deploy',
        lazy_callback(
            'awscli.customizations.codedeploy.codedeploy', 'inject_commands'
        ),
    )
    event_handlers.register(
        'building-argument-table.deploy.get-application-revision',
        lazy_callback(
            'awscli.customizations.codedeploy.locationargs',
            'modify_revision_arguments',
        ),
    )
    event_handlers.register(
        'building-argument-table.deploy.register-application-revision',
        lazy_callback(
            'awscli.customizations.codedeploy.locationargs',
            'modify_revision_arguments',
        ),
    )
    event_handlers.register(
        'building-argument-table.deploy.create-deployment',
        lazy_callback(
            'awscli.customizations.codedeploy.locationargs',
            'modify_revision_arguments',
        ),
    )

    # gamelift (~1ms)
    event_handlers.register(
        'building-command-table.gamelift',
        lazy_callback('awscli.customizations.gamelift', 'inject_commands'),
    )

    # servicecatalog (~3ms)
    event_handlers.register(
        'building-command-table.servicecatalog',
        lazy_callback(
            'awscli.customizations.servicecatalog', 'inject_commands'
        ),
    )

    # dlm (~2ms)
    event_handlers.register(
        'building-command-table.dlm',
        lazy_callback('awscli.customizations.dlm.dlm', 'register_commands'),
    )

    # ssm session (~1ms)
    event_handlers.register(
        'building-command-table.ssm',
        lazy_callback(
            'awscli.customizations.sessionmanager',
            'add_custom_start_session',
        ),
    )

    # logs (~3ms)
    event_handlers.register(
        'building-command-table.logs',
        lazy_callback(
            'awscli.customizations.logs', 'inject_tail_command'
        ),
    )
    event_handlers.register(
        'building-command-table.logs',
        lazy_callback(
            'awscli.customizations.logs', 'inject_start_live_tail_command'
        ),
    )

    # ec2-instance-connect (~5ms)
    event_handlers.register(
        'building-command-table.ec2-instance-connect',
        lazy_callback(
            'awscli.customizations.ec2instanceconnect', 'inject_commands'
        ),
    )

    # dsql (~0.5ms)
    event_handlers.register(
        'building-command-table.dsql',
        lazy_callback(
            'awscli.customizations.dsql',
            '_add_generate_dsql_db_connect_auth_token',
        ),
    )
    event_handlers.register(
        'building-command-table.dsql',
        lazy_callback(
            'awscli.customizations.dsql',
            '_add_generate_dsql_db_connect_admin_auth_token',
        ),
    )

    # cloudwatch otel rename (~0.5ms)
    event_handlers.register(
        'building-command-table.cloudwatch',
        lazy_callback(
            'awscli.customizations.cloudwatch', 'rename_otel_commands'
        ),
    )
