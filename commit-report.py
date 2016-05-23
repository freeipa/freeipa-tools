#!/usr/bin/python3

"""Generate a commit report

Usage:
  commit-report.py [options] [-v...] [-c KEY=VAL...]

Options:
  --since=WHEN            Start date [default: one.month.ago]
  --until=WHEN            End commit
  --from=COMMIT           Starting commit
  --to=COMMIT             Ending commit (defaults to {remote}/master)
  -h, --help              Display this help and exit
  -v, --verbose           Increase verbosity
  --config FILE           Configuration file [default: ~/.ipa/pushpatch.yaml]
  -c, --set-conf=KEY=VAL  Set a config option(s). VAL is in YAML format.
  --no-trac               Do not contact Trac
  --no-fetch              Do not synchronize before reporting
  --mailto=EMAIL          Mail results to this address
  --mailfrom=EMAIL        Mail results from this address
  --color=(auto|always|never)  Colorize output [default: auto]

Generate a commit report.

The --mailto and --mailfrom options must be specified together.

Configuration can be specified in the file given by --config.
Here is an example
  project-name: FreeIPA
  clean-repo-path: ~/dev/freeipa-clean
  ticket-url: https://fedorahosted.org/freeipa/ticket/
  commit-url: https://fedorahosted.org/freeipa/changeset/
  bugzilla-bug-url: https://bugzilla.redhat.com/show_bug.cgi?id=
  trac-xmlrpc-url: https://fedorahosted.org/freeipa/xmlrpc
  remote: origin
  smtp-host: smtp.example.com
  smtp-port: 25
"""

# TODO: Much of this is copied from pushpatches.py. Make a common library.

import os
import sys
import subprocess
import re
import collections
import pprint
import xmlrpc.client
import functools
import datetime
import smtplib
import math
from email.mime.text import MIMEText
from email.mime.multipart import MIMEMultipart
import csv

import docopt     # yum install python3-docopt
import yaml       # yum install python3-PyYAML
import blessings  # yum install python3-blessings
import pytz       # yum install python3-pytz

COLOR_OPT_MAP = {'auto': False, 'always': True, 'never': None}

SubprocessResult = collections.namedtuple(
    'SubprocessResult', 'stdout stderr returncode')
TracTicketData = collections.namedtuple(
    'TracTicketData', 'id time_created time_changed attributes')
CommitGroup = collections.namedtuple(
    'CommitGroup', 'info commits')
AuthorInfo = collections.namedtuple(
    'AuthorInfo', 'name email date')


class reify(object):
    # https://github.com/Pylons/pyramid/blob/1.4-branch/pyramid/decorator.py
    def __init__(self, wrapped):
        self.wrapped = wrapped
        self.__doc__ = getattr(wrapped, '__doc__', None)

    def __get__(self, inst, objtype=None):
        if inst is None:
            return self
        val = self.wrapped(inst)
        setattr(inst, self.wrapped.__name__, val)
        return val


@functools.total_ordering
class Ticket(object):
    """Trac ticket with lazily fetched information"""
    _memo = {}

    def __init__(self, cli, number):
        self.cli = cli
        self.number = number

    @reify
    def data(self):
        try:
            return self._memo[self]
        except KeyError:
            self.cli.eprint('Retrieving ticket %s' % self.number)
            data = TracTicketData(*self.cli.trac.ticket.get(self.number))
            self._memo[self] = self
            return data

    @property
    def attributes(self):
        return self.data.attributes

    @property
    def url(self):
        return self.cli.config['ticket-url'] + str(self.number)

    def __hash__(self):
        return hash(self.number) ^ hash(self.cli)

    def __eq__(self, other):
        return self.number == other.number

    def __lt__(self, other):
        return self.number < other.number


class CLIHelper(object):
    def __init__(self, options):
        self.output = []
        self.options = options
        with open(os.path.expanduser(options['--config'])) as conf_file:
            self.config = yaml.safe_load(conf_file)
        for opt in options['--set-conf']:
            key, sep, value = opt.partition('=')
            self.config[key] = yaml.safe_load(value)
        self.term = blessings.Terminal(
            force_styling=COLOR_OPT_MAP[options['--color']])
        self.verbosity = self.options['--verbose']
        if self.verbosity:
            self.eprint('Options:')
            self.eprint(pprint.pformat(self.options))
            self.eprint('Config:')
            self.eprint(pprint.pformat(self.config))
        if self.options['--no-trac']:
            self.trac = None
        else:
            url = self.config['trac-xmlrpc-url']
            self.trac = xmlrpc.client.ServerProxy(url)

        self.color_arg = self.options['--color']
        if self.color_arg == 'auto':
            if self.term.is_a_tty:
                self.color_arg = 'always'
            else:
                self.color_arg = 'never'

    def die(self, message):
        self.eprint(self.term.red(message), file=sys.stderr)
        exit(1)

    def eprint(self, *messages, **kwargs):
        """Print to stderr"""
        kwargs.setdefault('file', sys.stderr)
        self.print(*messages, **kwargs)

    def print(self, *objects, sep=' ', end='\n', file=sys.stdout):
        if file == sys.stdout:
            self.output.append(sep.join(str(o) for o in objects) + end)
        print(*objects, sep=sep, end=end, file=file)

    def write(self, string, file=sys.stdout):
        self.output.append(string)
        print(string, end='')
        sys.stdout.flush()

    def runcommand(self, argv, *, check_stdout=None, check_stderr=None,
                   check_returncode=0, stdin_string='', fail_message=None,
                   timeout=5, verbosity=None):
        """Run a command in a subprocess, check & return result"""
        argv_repr = ' '.join(self.shell_quote(a) for a in argv)
        if verbosity is None:
           verbosity = self.verbosity 
        if verbosity:
            self.eprint('+', self.term.blue(argv_repr))
        if verbosity > 2:
            self.eprint(self.term.yellow(stdin_string.rstrip()))
        PIPE = subprocess.PIPE
        proc = subprocess.Popen(argv, stdout=PIPE, stderr=PIPE, stdin=PIPE)
        try:
            stdout, stderr = proc.communicate(stdin_string.encode('utf-8'),
                                             timeout=timeout)
            timeout_expired = False
        except subprocess.TimeoutExpired:
            proc.kill()
            stdout = stderr = b''
            timeout_expired = True
        stdout = stdout.decode('utf-8')
        stderr = stderr.decode('utf-8')
        returncode = proc.returncode
        failed = any([
            timeout_expired,
            (check_stdout is not None and check_stdout != stdout),
            (check_stderr is not None and check_stderr != stderr),
            (check_returncode is not None and check_returncode != returncode),
        ])
        if failed and not verbosity:
            self.eprint('+', self.term.blue(argv_repr))
        if failed or verbosity >= 2:
            if stdout:
                self.eprint(stdout.rstrip())
            if stderr:
                self.eprint(self.term.yellow(stderr.rstrip()))
            self.eprint('→ %s' % self.term.blue(str(proc.returncode)))
        if failed:
            if timeout_expired: 
                self.die('Command timeout expired')
            elif fail_message:
                self.die(fail_message)
            else:
                self.die('Command failed')
        return SubprocessResult(stdout, stderr, returncode)

    @staticmethod
    def shell_quote(arg):
        """Quote an argument for the shell"""
        if re.match('^[-_.:/=a-zA-Z0-9]*$', arg):
            return arg
        else:
            return "'%s'" % arg.replace("'", r"'\''")

    @staticmethod
    def cleanpath(path):
        """Return absolute path with leading ~ expanded"""
        path = os.path.expanduser(path)
        path = os.path.abspath(path)
        return path

    def pprint(text, language=None):
        'TODO'


def groupby(commits, get_groupkeys,
            get_sortkey=lambda group: (-group.info['num_commits'],
                                       -group.info['changed'])):
    groups = {}
    for commit in commits:
        for key in get_groupkeys(commit):
            group = groups.setdefault(key, CommitGroup({}, []))
            group.commits.append(commit)
    for key, group in groups.items():
        group.info['key'] = key
        group.info['files'] = {}
        group.info['added'] = 0
        group.info['removed'] = 0
        group.info['tickets'] = set()
        group.info['num_commits'] = len(group.commits)
        for commit in group.commits:
            for filename, (added, removed) in commit.files.items():
                add1, rem1 = group.info['files'].get(filename, (0, 0))
                add1 += added
                rem1 += removed
                group.info['added'] += added
                group.info['removed'] += removed
                group.info['files'][filename] = add1, rem1
            group.info['tickets'].update(commit.tickets)
        group.info['changed'] = group.info['added'] + group.info['removed']
    return sorted(groups.values(), key=get_sortkey)


class Commit(object):
    def __init__(self, sha1):
        self.sha1 = sha1
        self.message = []
        self.parents = []
        self.tickets = set()
        self.files = {}
        self.reviewers = []

    @staticmethod
    def parse_commit_info(line):
        match = re.match(r'(.+ <[^>]+>) (\d+) ([+-])(\d\d)(\d\d)', line)
        if not match:
            print(line)
        offset = 60 * int(match.group(4)) + int(match.group(5))
        if match.group(3) == '-':
            offset = -offset
        date = datetime.datetime.fromtimestamp(int(match.group(2)),
                                               pytz.FixedOffset(offset))
        name_mail = check_mailmap(match.group(1))
        name, _sep, mail = name_mail.partition(' <')
        mail = mail.strip('>')
        return AuthorInfo(name, mail, date)

    @property
    def reviewers_short(self):
        return ','.join(shorten_author(r) for r in self.reviewers)

    @property
    def author(self):
        return '%s <%s>' % (self.author_info.name, self.author_info.email)

    @property
    def commit_date(self):
        return self.committer_info.date

    @property
    def author_short(self):
        return shorten_author(self.author)

    @property
    def added(self):
        return sum(a for a, r in self.files.values())

    @property
    def removed(self):
        return sum(r for a, r in self.files.values())

    @property
    def summary(self):
        return self.message[0]


def shorten_author(author):
    match = re.match(r'^.*<(.*)@.*> *$', author)
    if match:
        return match.group(1)
    else:
        return author


def git_date(cli, spec):
    output = cli.runcommand(['git', 'rev-parse', '--since=%s' % spec]).stdout.strip()
    name, sep, seconds = output.partition('=')
    return datetime.datetime.fromtimestamp(int(seconds))


def git_describe(cli, commit):
    argv = ['git', 'describe', '--tags', commit]
    return cli.runcommand(argv).stdout.strip()


def check_mailmap(name):
    cmd = cli.runcommand(
        ['git', '-c', 'mailmap.blob=origin/master:.mailmap',
            'check-mailmap', name],
        check_returncode=None)
    if cmd.returncode == 0:
        return cmd.stdout.strip()
    else:
        return name + ' <bad-entry@invalid>'


def safelogdiv(numerator, denominator):
    if not denominator:
        return 'inf'
    if not numerator:
        return '-inf'
    else:
        return math.log(numerator / denominator)


def safediv(numerator, denominator):
    if not denominator:
        return 'inf'
    else:
        return numerator / denominator


def csv_num_repr(number):
    if isinstance(number, (int, float)):
        if number == int(number):
            return int(number)
        elif abs(number) > 1:
            return round(number, 2)
        else:
            return round(number, int(-math.log(abs(number), 10) + 3))
    else:
        return number


def run(cli):
    cli.print('Generated:', datetime.datetime.now())

    os.chdir(cli.cleanpath(cli.config['clean-repo-path']))

    remote = cli.config['remote']
    if not cli.options['--no-fetch']:
        cli.eprint('Fetching...')
        cli.runcommand(['git', 'fetch', remote], timeout=60)

    log_args = []
    end_commit = cli.options['--to'] or '%s/master' % remote
    start_commit = cli.options['--from']
    if start_commit:
        log_args.append('%s..%s' % (start_commit, end_commit))
        cli.print('From:     ', start_commit,
              '(%s)' % git_describe(cli, start_commit))
    else:
        log_args.append(end_commit)
    cli.print('To:       ', end_commit, '(%s)' % git_describe(cli, end_commit))

    since = cli.options['--since']
    if since:
        log_args.append('--since=%s' % since)
        cli.print('Since:    ', git_date(cli, since))
    until = cli.options['--until']
    if until:
        log_args.append('--until=%s' % until)
        cli.print('Until:    ', git_date(cli, until))
    cli.print()

    static_argv = ['git', '-c', 'mailmap.blob=origin/master:.mailmap',
                   'log', '--boundary', '--format=raw', '--numstat',
                   '--use-mailmap']
    output = cli.runcommand(static_argv + log_args, timeout=60).stdout

    ticket_re = re.compile(re.escape(cli.config['ticket-url']) + '(\d+)')

    all_tickets = set()
    commits = []
    boundary_commits = []
    on_gpgsig = False

    for line in output.splitlines():
        header, sep, content = line.partition(' ')
        content = content.strip()
        if not line:
            # gpgsig is multi-line field, starting with ' '
            on_gpgsig = False
        elif header == 'commit':
            current_commit = Commit(content.strip(' -'))
            if content.startswith('-'):
                boundary_commits.append(current_commit)
            else:
                commits.append(current_commit)
        elif header == 'tree':
            current_commit.tree = content
        elif header == 'parent':
            current_commit.parents.append(content)
        elif header == 'author':
            current_commit.author_info = Commit.parse_commit_info(content)
        elif header == 'committer':
            current_commit.committer_info = Commit.parse_commit_info(content)
        elif line.startswith('    '):
            current_commit.message.append(line[4:].rstrip('\n'))
            for match in ticket_re.finditer(line):
                ticket = Ticket(cli, int(match.group(1)))
                all_tickets.add(ticket)
                current_commit.tickets.add(ticket)
            mheader, sep, mcontent = line.strip().partition(' ')
            if mheader.lower() == 'reviewed-by:':
                current_commit.reviewers.append(check_mailmap(mcontent))
        elif line.startswith('gpgsig') or on_gpgsig:
            on_gpgsig = True
        else:
            added, removed, filename = line.split('\t')
            try:
                added_removed = int(added), int(removed)
            except ValueError:
                print(line)
                added_removed = 0, 0
            current_commit.files[filename] = added_removed

    cli.print('git log:', ' '.join(cli.shell_quote(arg) for arg in log_args))
    for commit in commits:
        cli.print('* {c.sha1:7.7} ({c.commit_date}) [{c.author_short};{c.reviewers_short}] {c.message[0]}'.format(c=commit))
    for commit in boundary_commits:
        cli.print('^ {c.sha1:7.7} ({c.commit_date}) [{c.author_short};{c.reviewers_short}] {c.message[0]}'.format(c=commit))
    cli.print(len(commits), 'commits')
    cli.print()

    rep = lambda title, key: print_report(cli, commits, title, key)

    rep('By patch author', lambda commit: [commit.author])
    rep('By reviewer', lambda commit: commit.reviewers)

    report_text = cli.output
    cli.output = []

    print(cli.term.cyan('=== CSV report ==='))

    if until:
        subject_date = git_date(cli, until).date()
    else:
        subject_date = datetime.date.today()
    if commits:
        commmit_range = '%s..%s' % (boundary_commits[0].sha1[:7],
                                    commits[0].sha1[:7])
    else:
        # Use empty trees
        empty_tree = cli.runcommand(['git', 'hash-object',
                                     '-t', 'tree', '/dev/null']).stdout
        commmit_range = '{0}..{0}'.format(empty_tree)

    outputter = csv.writer(cli)
    outputter.writerow(['id', 'summary', '+', '-',
                        'author', 'reviewer'])
    for commit in commits:
        outputter.writerow([commit.sha1[:7], commit.summary,
                            commit.added, commit.removed,
                            commit.author] +
                           list(commit.reviewers))

    main_csv_text = cli.output
    cli.output = []

    print(cli.term.cyan('=== People report ==='))

    outputter = csv.writer(cli)
    outputter.writerow(['name', 'patches', 'reviews',
                        'lines changed', 'lines reviewed',
                        'patches:reviews',
                        'lines changed:reviewed'])
    people = set(c.author for c in commits)
    for commit in commits:
        people.update(commit.reviewers)
    people_info = {p: {'authored': [c for c in commits if c.author == p],
                       'reviewed': [c for c in commits if p in c.reviewers]}
                   for p in people}
    # In case of multiple reviewers, the patch is counted for all of them,
    # but the lines changed are split between the reviewers.
    for info in people_info.values():
        info['lines_authored'] = sum(c.added + c.removed
                                     for c in info['authored'])
        info['lines_reviewed'] = sum((c.added + c.removed) / len(c.reviewers)
                                     for c in info['reviewed'])
    def sort_key(k_v):
        k, v = k_v
        if v['reviewed']:
            backup_key = -len(v['authored']), -len(v['reviewed'])
            backup_key += -v['lines_authored'], -v['lines_reviewed']
            return 0, len(v['authored']) / len(v['reviewed']), backup_key
        else:
            return 1, len(v['authored']), v['lines_authored']
    for person, info in sorted(people_info.items(), key=sort_key):
        outputter.writerow([
            person,
            len(info['authored']),
            len(info['reviewed']),
            info['lines_authored'],
            csv_num_repr(info['lines_reviewed']),
            csv_num_repr(safediv(len(info['authored']), len(info['reviewed']))),
            csv_num_repr(safediv(info['lines_authored'], info['lines_reviewed'])),
        ])

    people_csv_text = cli.output
    cli.output = []

    if cli.options['--mailto']:
        project_name = cli.config['project-name']

        msg = MIMEMultipart('mixed')

        text_part = MIMEText(''.join(report_text), 'plain')
        msg.attach(text_part)

        csv_part = MIMEText(''.join(main_csv_text), 'csv')
        csv_part.add_header('Content-Disposition',
                            'attachment; filename="{}-commits-{}.csv"'.format(
                                project_name.lower(),
                                subject_date.strftime('%Y-%m')))
        msg.attach(csv_part)

        ppl_part = MIMEText(''.join(people_csv_text), 'csv')
        ppl_part.add_header('Content-Disposition',
                            'attachment; filename="{}-people-{}.csv"'.format(
                                project_name.lower(),
                                subject_date.strftime('%Y-%m')))
        msg.attach(ppl_part)

        msg['Subject'] = '{project} commit report {date} ({commmit_range})'.format(
            project=project_name,
            date=subject_date,
            commmit_range=commmit_range)
        msg['From'] = cli.options['--mailfrom']
        to = cli.options['--mailto']
        msg['To'] = to

        host = cli.config['smtp-host']
        port = int(cli.config['smtp-port'])
        cli.eprint('Sending results to {to} via {host}:{port}...'.format(
            to=to,
            host=host,
            port=port))
        smtp = smtplib.SMTP(host, port)
        smtp.send_message(msg)
        smtp.quit()
        cli.eprint('Sent!')


def print_report(cli, commits, title, keyfunc):
    cli.print('%s:' % title)
    for group in groupby(commits, keyfunc):
        cli.print('{g.info[num_commits]:4}  {g.info[key]} (+{g.info[added]} -{g.info[removed]})'.format(g=group))
        for commit in group.commits:
            cli.print('        {c.sha1:7.7} [{c.author_short};{c.reviewers_short}] {c.summary} (+{c.added} -{c.removed})'.format(url=cli.config['commit-url'], c=commit))
        for ticket in sorted(group.info['tickets']):
            if cli.trac:
                if ticket.attributes['status'] == 'closed':
                    cli.print('        #{t.number} [{t.attributes[owner]}\'s {t.attributes[component]} {t.attributes[type]}] {t.attributes[summary]}'.format(t=ticket))
                    if ticket.attributes['rhbz'] and ticket.attributes['rhbz'] != '0':
                        rhbz = ticket.attributes['rhbz'].split(' ', 1)[-1].strip('[]')
                        cli.print('          BZ {rhbz}'.format(rhbz=rhbz))
            else:
                cli.print('        {t.url}'.format(t=ticket))
    cli.print()

if __name__ == '__main__':
    cli = CLIHelper(docopt.docopt(__doc__))
    run(cli)
