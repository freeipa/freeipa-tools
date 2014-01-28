#! /usr/bin/env python2

"""Format FreeIPA declarative tests as Mediawiki text for freeipa.org

Usage: get-tests [options] <test>...

Options:
    --run                   Actually run the tests while gathering info
    --permission-acishow    After aci_show commands, print permission info
                                (requires --run)

<test> may be a Python module name; if it doesn't contain a dot,
'ipatests.test_xmlrpc.' is prepended.
All Declarative tests defined in that module are converted.
Optionally it might be followed by a ':' and a comma-separated list of
test classes; in this case only those tests are converted.

Note that this is a one-off hack. Functionality is only added as needed.
"""

import pprint
import re
import textwrap
import contextlib
import StringIO
import sys
import copy
import json

import docopt  # sudo dnf install python-docopt

from ipalib import parameters
from ipalib import api, errors
from ipalib.cli import cli_plugins
from ipapython import ipautil, ipaldap
from ipapython.dn import DN
from ipalib.rpc import json_encode_binary

argv = list(sys.argv[1:])
del sys.argv[1:]

api.bootstrap_with_global_options(context='cli')
api.load_plugins()
for cls in cli_plugins:
    api.register(cls)
api.finalize()

try:
    from ipalib.plugins.permission import DNOrURL
except ImportError:
    class DNOrURL(object): pass

def shell_quote(string):
    if re.match('^[-._~a-zA-Z0-9]+$', string):
        return string
    else:
        return ipautil.shell_quote(string)


def unparse_param(param, value):
    if isinstance(value, (list, tuple)):
        if len(value) == 0:
            return ''
        elif len(value) == 1:
            return unparse_param(param, value[0])
        else:
            return '{%s}' % ','.join(unparse_param(param, v) for v in value)
    elif type(param) in (parameters.StrEnum, parameters.Str,
                       parameters.DNParam, DNOrURL, parameters.Int):
        return shell_quote(unicode(value))
    elif type(param) in (parameters.Flag, ):
        return None
    else:
        raise TypeError(type(param))


TEMPLATES = {
    ('aci_show', True): textwrap.dedent("""
            {{{{Test Case|{name}
            |setup=See beginning of the Tests section
            |actions=Search for ACI named <tt>{opts[aciprefix]}:{args[0]}</tt> in <tt>{opts[location]}</tt>
            |results=The following ACI is found:
             <nowiki>{expected[result][aci]}</nowiki>
            }}}}
        """),
    ('aci_show', False): textwrap.dedent("""
            {{{{Test Case|{name}
            |setup=See beginning of the Tests section
            |actions=Search for ACI named <tt>{opts[aciprefix]}:{args[0]}</tt> in <tt>{opts[location]}</tt>
            |results=Such ACI is not found.
            }}}}
        """),
}


DEFAULT_TEMPLATE = textwrap.dedent("""
    {{{{Test Case|{name}
    |setup=See beginning of the Tests section
    |actions=Run the following command:
     {actions}
    |results=
    {results}
    }}}}
""")

NOCLI_TEMPLATE = textwrap.dedent("""
    {{{{Test Case|{name}
    |setup=See beginning of the Tests section
    |actions=Issue the following through the JSON API:
     {json_in}
    |results=The response is:
     {json_out}
    }}}}
""")


@contextlib.contextmanager
def capture_stdout():
    io = StringIO.StringIO()
    old_stdout = sys.stdout
    sys.stdout = io
    yield io
    sys.stdout = old_stdout


def get_commandline(cmd_tuple):
    cmd_name, args, opts = cmd_tuple
    commandline_parts = ['ipa']
    commandline_parts.append(cmd_name)
    command = api.Command[cmd_name]
    for varg, oarg in zip(args, command.args()):
        commandline_parts.append(unparse_param(oarg, varg))
    for oname, ovalue in opts.items():
        oopt = command.options[oname]
        pval = unparse_param(oopt, ovalue)
        if pval is None:
            commandline_parts.append('--%s' % oopt.cli_name)
        else:
            commandline_parts.append('--%s=%s' % (oopt.cli_name,
                                                  pval))
    return ' '.join(commandline_parts)

def to_json(dct, method=None):
    try:
        dct = json_encode_binary(dct)
        if method:
            dct['method'] = method
        result = json.dumps(dct, ensure_ascii=False, indent=2, sort_keys=True)
        return result.replace('\n', '\n ')
    except TypeError, e:
        class Bad(object):
            def __str__(self):
                raise e
        return Bad()


def make_wikitests(declarative_test_class, run=False, permission_acishow=False):
    yield '<h2>%s</h2>' % declarative_test_class.__name__.replace('_', ' ').capitalize()
    yield 'Implemented in <tt>%s.%s</tt>' % (
        declarative_test_class.__module__,
        declarative_test_class.__name__)

    yield ''
    if declarative_test_class.__doc__:
        yield declarative_test_class.__doc__
        yield ''

    yield 'Like other tests in the test_xmlrpc suite, these tests should run '
    yield 'on a clean IPA installation, or possibly after other similar tests.'

    if run:
        ldap = ipaldap.IPAdmin()

        for cmd_name, args, opts in declarative_test_class.cleanup_commands:
            print '<!-- Run %s %s %s -->' % (cmd_name, args, opts)
            try:
                api.Command[cmd_name](*args, **opts)
            except Exception, e:
                print '<!-- %s: Err, %s: %s -->' % (cmd_name,
                                                    type(e).__name__, e)
            else:
                print '<!-- %s: OK -->' % cmd_name

    for test in declarative_test_class.tests:
        if callable(test):
            print '{{:{{subst:FULLPAGENAME}}/%s}}' % test.__name__
            if run:
                test(None)
            continue
        cmd_name, args, opts = test['command']
        command = api.Command[cmd_name]

        if run:
            try:
                command(*args, **opts)
            except Exception, e:
                print '<!-- %s: %s -->' % (cmd_name, type(e).__name__)
            else:
                print '<!-- %s: OK -->' % cmd_name

        expected = test['expected']
        if isinstance(expected, Exception):
            result = 'The command fails with this error:\n %s' % expected.strerror
            success = False
        else:
            with capture_stdout() as stdout:
                rv = command.output_for_cli(api.Backend.textui,
                                            copy.deepcopy(expected),
                                            *args, **opts)
            outvalue = stdout.getvalue()
            outvalue = outvalue
            result = 'The command %s with this output:%s' % (
                ('fails (return code %s),' % rv if rv else 'succeeds'),
                ('\n' + outvalue.strip('\n')).replace('\n', '\n '))
            success = (rv == 0)

        default_template = NOCLI_TEMPLATE if command.NO_CLI else DEFAULT_TEMPLATE
        yield TEMPLATES.get((cmd_name, success), default_template).format(
            name=test['desc'].partition(' #(')[0],
            actions=get_commandline(test['command']),
            results=result,
            cmd_name=cmd_name,
            args=args,
            opts=opts,
            expected=expected,
            json_in=to_json(dict(params=[args, opts], method=cmd_name, id=0),
                            method=cmd_name),
            json_out=to_json(expected),
        ).replace(str(api.env.basedn), '$SUFFIX')


        if run and permission_acishow and cmd_name == 'aci_show':
            dn = DN(('cn', args[0]), api.env.container_permission, api.env.basedn)
            yield '<noinclude>'
            yield ''
            dn_display = str(dn).replace(str(api.env.basedn), '$SUFFIX')
            try:
                entry = ldap.get_entry(dn)
            except errors.NotFound:
                yield 'Note: the permission entry %s will not be present' % dn_display
            else:
                yield 'Note: the permission entry will look like this:'
                yield ''
                yield ' dn: %s' % dn_display
                for attrname, values in sorted(entry.items()):
                    for value in sorted(values):
                        yield ' %s: %s' % (attrname, str(value).replace(str(api.env.basedn), '$SUFFIX'))
            yield '</noinclude>'
            yield ''

    yield '<h3>Cleanup</h3>'
    for cmd_tuple in declarative_test_class.cleanup_commands:
        yield ' %s' % get_commandline(cmd_tuple)
    yield ''
    yield ''


def get_class_order_key(module, clsname):
    try:
        with open(module.__file__) as f:
            return f.read().find(clsname)
    except Exception, e:
        return 1, '%s: %s' % (type(e).__name, e)


if __name__ == '__main__':
    from ipatests.test_xmlrpc.xmlrpc_test import Declarative
    opts = docopt.docopt(__doc__, argv)
    print '__NOTOC__'
    print '<!-- generated with: %s -->' % argv
    for module_name in opts['<test>']:
        module_name, sep, cls_names = module_name.partition(':')
        cls_names = [n for n in cls_names.split(',') if n]
        if '.' not in module_name:
            module_name = 'ipatests.test_xmlrpc.' + module_name
        __import__(module_name)
        module = sys.modules[module_name]
        if not cls_names:
            for cls in vars(module).values():
                if (isinstance(cls, type) and issubclass(cls, Declarative) and
                        cls.__module__ == module_name):
                    cls_names.append(cls.__name__)
        cls_names.sort(key=lambda n: get_class_order_key(module, n))
        for cls_name in cls_names:
            for s in make_wikitests(
                    getattr(module, cls_name),
                    run=opts['--run'],
                    permission_acishow=opts['--permission-acishow'],
                    ):
                print s
