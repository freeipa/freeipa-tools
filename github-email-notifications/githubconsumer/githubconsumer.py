#!/usr/bin/python2
# inspired by: https://github.com/lmacken/fedmsg-koji-consumer

from __future__ import print_function

import email
import fedmsg.consumers
import logging
import smtplib
import StringIO
import urllib2

from systemd import journal
from abc import ABCMeta, abstractmethod, abstractproperty
from email.header import Header
from email.mime.multipart import MIMEMultipart
from email.mime.text import MIMEText
from pprint import pprint

def get_from_dict(data_dict, key_list):
    return reduce(lambda d, k: d[k], key_list, data_dict)


class Formatter(object):
    __metaclass__ = ABCMeta

    @abstractmethod
    def fmt_issue_comment(self, comment):
        output = StringIO.StringIO()
        output.write(u"  URL: {pr_url}\n".format(**comment))
        output.write(u"Title: #{issue_num}: {issue_title}\n".format(**comment))
        output.write(u"\n")
        output.write(
            u"{comment_author} commented:\n".format(**comment)
        )
        output.write(u'"""\n')
        output.write(comment['comment_body'])
        output.write(u'\n')
        output.write(u'"""\n')
        output.write(u'\n')
        output.write(
            u"See the full comment at {comment_url}\n".format(**comment)
        )
        res = output.getvalue()
        output.close()
        return res

    @abstractmethod
    def fmt_pr(self, pull_req):
        output = StringIO.StringIO()
        output.write(u"   URL: {pr_url}\n".format(**pull_req))
        output.write(u"Author: {pr_author}\n".format(**pull_req))
        output.write(u" Title: #{pr_num}: {pr_title}\n".format(**pull_req))
        output.write(u"Action: {pr_action_txt}\n".format(**pull_req))
        output.write(u'\n')
        if pull_req['pr_action'] == u'opened':
            output.write(u"PR body:\n")
            output.write(u'"""\n')
            output.write(u"{pr_body}\n".format(**pull_req))
            output.write(u'"""\n')
            output.write(u'\n')
        output.write(
            u"To pull the PR as Git branch:\n"
            u"git remote add gh{project} {repo_url_ro}\n"
            u"git fetch gh{project} pull/{pr_num}/head:pr{pr_num}\n"
            u"git checkout pr{pr_num}\n".format(
                project=self.project, **pull_req
            )
        )
        res = output.getvalue()
        output.close()
        return res

    @abstractmethod
    def fmt_pr_edited(self, msg):
        output = StringIO.StringIO()
        output.write(u"   URL: {pr_url}\n".format(**msg))
        output.write(u"Author: {pr_author}\n".format(**msg))
        output.write(u" Title: #{pr_num}: {pr_title}\n".format(**msg))
        output.write(u"Action: {pr_action_txt}\n".format(**msg))
        output.write(u"\n")
        for field, original_value in msg['pr_changes'].items():
            output.write(u" Changed field: {}\n".format(field))
            output.write(u"Original value:\n")
            output.write(u'"""\n')
            output.write(u"{}\n".format(original_value['from']))
            output.write(u'"""\n')
            output.write(u"\n")
        res = output.getvalue()
        output.close()
        return res

    @abstractmethod
    def fmt_labeled(self, comment):
        output = StringIO.StringIO()
        if comment['pr_action'] not in {u'labeled', u'unlabeled'}:
            assert False, "Unexpected pr_action '{}'".format(comment['pr_action'])

        output.write(u"  URL: {pr_url}\n".format(**comment))
        output.write(u"Title: #{pr_num}: {pr_title}\n".format(**comment))
        output.write(u"\n")
        output.write(u"Label: {action_prefix}{pr_label}\n".format(**comment))
        res = output.getvalue()
        output.close()
        return res


class EmailFormatter(Formatter):
    def __init__(self, project, to_addr, from_addr, smtp_server, log=None):
        """

        :param to_addr: email destinations
        :param from_addr: source email address
        """
        super(EmailFormatter, self).__init__()

        self.project = project
        self.to_addr = to_addr
        self.from_addr = from_addr
        self.smtp_server = smtp_server
        self.domain = from_addr.replace('@', '.')
        self.log = log if log else logging.getLogger()

    def _msg_id(self, repo, gh_msgid, pr_num):
        msgid = "<gh-{repo}-{pr_num}-{gh_msgid}@{domain}>".format(
            repo=repo, gh_msgid=gh_msgid, pr_num=pr_num, domain=self.domain
        )
        threadid = "<gh-{repo}-{pr_num}@{domain}>".format(
            repo=repo, pr_num=pr_num, domain=self.domain
        )
        return msgid, threadid

    def _send_email(self, from_login, subject, body, msgid, threadid,
                    attachments=(), new_thread=False):
        """
        Inspired by: https://github.com/abartlet/gh-mailinglist-notifications/blob/master/gh-mailinglist.py
        """
        subject = subject.replace('\n', ' ').replace('\r', ' ')
        outer = MIMEMultipart()
        outer['Subject'] = Header(subject, 'UTF-8')
        outer['To'] = self.to_addr
        outer['X-githubconsumer-project'] = self.project
        outer['X-githubconsumer-author-gh-login'] = from_login
        outer['From'] = "{name} <{noreplyaddr}>".format(
                name=from_login, noreplyaddr=self.from_addr
            )

        if new_thread:
            outer.add_header("Message-ID", threadid)
        else:
            outer.add_header("Message-ID", msgid)
            outer.add_header("In-Reply-To", threadid)
            outer.add_header("References", threadid)

        outer['Date'] = email.utils.formatdate(localtime=True)
        outer.attach(MIMEText(body, 'plain', 'UTF-8'))

        for filename, data in attachments:
            msg = MIMEText(data, 'x-diff', 'UTF-8')
            msg.add_header('Content-Disposition', 'attachment',
                filename=filename)
            outer.attach(msg)

        msg = outer.as_string()
        s = smtplib.SMTP(self.smtp_server)
        s.sendmail(self.from_addr, [self.to_addr], msg)
        s.quit()
        self.log.info("Sent notification: \"%s\" to \"%s\"", subject, self.to_addr)

    def _get_patch(self, url):
        try:
            response = urllib2.urlopen(url)
            return response.read()
        except urllib2.HTTPError:
            self.log.exception("Cannot download patch: %s", url)

    def fmt_issue_comment(self, comment):
        body = super(EmailFormatter, self).fmt_issue_comment(comment)
        subject = u"[{project} PR#{issue_num}][comment] {issue_title}".format(
            project=self.project, **comment)
        msgid, threadid = self._msg_id(
            comment['repo'], comment['msgid'], comment['issue_num'])
        self._send_email(comment['event_author'], subject, body, msgid, threadid)

    def fmt_pr(self, comment):
        if comment['pr_action'] == 'synchronize':
            comment['pr_action_txt'] = u'synchronized'

        body = super(EmailFormatter, self).fmt_pr(comment)
        subject = u"[{project} PR#{pr_num}][{pr_action_txt}] {pr_title}".format(
            project=self.project, **comment)
        msgid, threadid = self._msg_id(
            comment['repo'], comment['msgid'], comment['pr_num'])

        attachments = ()

        if comment['pr_action'] in {'opened', 'synchronize'}:
            # send PR as patch in attachment
            patch_data = self._get_patch("{pr_url}.patch".format(**comment))
            if patch_data:
                attachments = [(
                    "{project}-pr-{pr_num}.patch".format(
                        project=self.project, **comment),
                    patch_data
                )]

        # warning if pullrequest was merged
        if comment['pr_merged'] and comment['pr_action'] == u"closed":
            subject = "WARNING: MERGED {}".format(subject)
            body = (
                "*WARNING: this pull request has been merged!*\n"
                "This is only mirrored repo thus any changes will be erased. "
                "Please push commit(s) to authoritative repository.\n\n"
                "{}".format(body)
            )

        if comment['pr_action'] == u'opened':
            new_thread = True
        else:
            new_thread = False

        self._send_email(
            comment['event_author'], subject, body, msgid,
            threadid, attachments=attachments, new_thread=new_thread)

    def fmt_labeled(self, comment):
        if comment['pr_action'] == u'labeled':
            comment['action_prefix'] = u'+'
        elif comment['pr_action'] == u'unlabeled':
            comment['action_prefix'] = u'-'
        body = super(EmailFormatter, self).fmt_labeled(comment)
        subject = u"[{project} PR#{pr_num}][{action_prefix}{pr_label}] {pr_title}".format(
            project=self.project, **comment)
        msgid, threadid = self._msg_id(
            comment['repo'], comment['msgid'], comment['pr_num'])
        self._send_email(comment['event_author'], subject, body, msgid, threadid)

    def fmt_pr_edited(self, comment):
        body = super(EmailFormatter, self).fmt_pr_edited(comment)
        subject = u"[{project} PR#{pr_num}][{pr_action}] {pr_title}".format(
            project=self.project, **comment)
        msgid, threadid = self._msg_id(
            comment['repo'], comment['msgid'], comment['pr_num'])
        self._send_email(comment['event_author'], subject, body, msgid,
            threadid)


class RawPPFormatter(Formatter):
    def fmt_issue_comment(self, comment):
        pprint(comment)

    def fmt_pr(self, pull_req):
        pprint(pull_req)


class GithubConsumer(fedmsg.consumers.FedmsgConsumer):

    __meta__ = ABCMeta

    topic = 'org.fedoraproject.prod.github.*'
    formatter_cls = EmailFormatter

    @abstractproperty
    def config_key(self):
        pass

    @abstractproperty
    def repo_name(self):
        pass

    @abstractproperty
    def project(self):
        pass

    def __init__(self, *args, **kw):
        super(GithubConsumer, self).__init__(*args, **kw)

        self.log = logging.getLogger(self.config_key)
        journal_handler = journal.JournalHandler()
        journal_handler.setFormatter(logging.Formatter(
            "[%(name)s %(levelname)s]: %(message)s"
        ))
        self.log.addHandler(journal_handler)
        if args[0].config.get("{}.debug".format(self.config_key)):
            self.log.setLevel(logging.DEBUG)
        else:
            self.log.setLevel(logging.INFO)

        self.topic_mapping = {
            'org.fedoraproject.prod.github.issue.comment': self.issue_comment,
            'org.fedoraproject.prod.github.issue.labeled': self.issue_labeled,
            'org.fedoraproject.prod.github.pull_request.edited': self.pr_edited,
            'org.fedoraproject.prod.github.pull_request.opened': self.pr_opened,
            'org.fedoraproject.prod.github.pull_request.reopened': self.pr_reopened,
            'org.fedoraproject.prod.github.pull_request.closed': self.pr_closed,
            'org.fedoraproject.prod.github.pull_request.synchronize': self.pr_synchronize,
            'org.fedoraproject.prod.github.pull_request.review_comment': self.pr_review,
            'org.fedoraproject.prod.github.pull_request.labeled': self.pr_labeled,
            'org.fedoraproject.prod.github.pull_request.unlabeled': self.pr_unlabeled,
            'org.fedoraproject.prod.github.status': self.status,
        }

        self.formatter = self.formatter_cls(
            self.project,
            args[0].config["{}.email_to".format(self.config_key)],
            args[0].config["{}.email_from".format(self.config_key)],
            args[0].config["{}.smtp_server".format(self.config_key)],
            log=self.log
        )

    def _format_msg(self, filter_map, gh_msg):
        msg = dict()
        for fld, klist in filter_map.iteritems():
            msg[fld] = get_from_dict(gh_msg, klist)
        return msg

    def _pr_handler(self, gh_msg):
        filter_map = {
            'event_author': ['body', 'msg', 'sender', 'login'],
            'msgid': ['body', 'msg_id'],
            'pr_url' : ['body', 'msg', 'pull_request', 'html_url'],
            'pr_author' : ['body', 'msg', 'pull_request', 'user', 'login'],
            'pr_title' : ['body', 'msg', 'pull_request', 'title'],
            'pr_body' : ['body', 'msg', 'pull_request', 'body'],
            'pr_merged': ['body', 'msg', 'pull_request', 'merged'],
            'pr_num' : ['body', 'msg', 'number'],
            'pr_action' : ['body', 'msg', 'action'],
            'pr_action_txt': ['body', 'msg', 'action'],
            'repo': ['body', 'msg', 'repository', 'full_name'],
            'repo_url_ro': ['body', 'msg', 'repository', 'html_url'],
        }
        msg = self._format_msg(filter_map, gh_msg)
        return self.formatter.fmt_pr(msg)

    def _pr_label_handler(self, gh_msg):
        filter_map = {
            'event_author': ['body', 'msg', 'sender', 'login'],
            'msgid': ['body', 'msg_id'],
            'pr_url' : ['body', 'msg', 'pull_request', 'html_url'],
            'pr_author' : ['body', 'msg', 'pull_request', 'user', 'login'],
            'pr_title' : ['body', 'msg', 'pull_request', 'title'],
            'pr_num' : ['body', 'msg', 'number'],
            'pr_label' : ['body', 'msg', 'label', 'name'],
            'pr_action': ['body', 'msg', 'action'],
            'repo': ['body', 'msg', 'repository', 'full_name'],
        }
        msg = self._format_msg(filter_map, gh_msg)
        return self.formatter.fmt_labeled(msg)

    def issue_comment(self, gh_msg):
        try:
            pr_link = get_from_dict(gh_msg, ['body', 'msg', 'issue', 'pull_request'])
        except KeyError:
            pr_link = None

        if not pr_link:
            # We only care about comments in pull-requests
            return

        filter_map = {
            'comment_url' : ['body', 'msg', 'comment', 'html_url'],
            'comment_author' : ['body', 'msg', 'comment', 'user', 'login'],
            'comment_body' : ['body', 'msg', 'comment', 'body'],
            'event_author': ['body', 'msg', 'sender', 'login'],
            'issue_num': ['body', 'msg', 'issue', 'number'],
            'issue_title': ['body', 'msg', 'issue', 'title'],
            'pr_url': ['body', 'msg', 'issue', 'html_url'],
            'msgid': ['body', 'msg_id'],
            'repo': ['body', 'msg', 'repository', 'full_name'],
        }
        msg = self._format_msg(filter_map, gh_msg)
        return self.formatter.fmt_issue_comment(msg)

    def issue_labeled(self, gh_msg):
        pass

    def pr_opened(self, gh_msg):
        return self._pr_handler(gh_msg)

    def pr_closed(self, gh_msg):
        return self._pr_handler(gh_msg)

    def pr_reopened(self, gh_msg):
        return self._pr_handler(gh_msg)

    def pr_review(self, msg):
        pass

    def pr_edited(self, gh_msg):
        filter_map = {
            'event_author': ['body', 'msg', 'sender', 'login'],
            'msgid': ['body', 'msg_id'],
            'pr_url' : ['body', 'msg', 'pull_request', 'html_url'],
            'pr_author' : ['body', 'msg', 'pull_request', 'user', 'login'],
            'pr_title' : ['body', 'msg', 'pull_request', 'title'],
            'pr_body' : ['body', 'msg', 'pull_request', 'body'],
            'pr_num' : ['body', 'msg', 'number'],
            'pr_action' : ['body', 'msg', 'action'],
            'pr_action_txt': ['body', 'msg', 'action'],
            'pr_changes': ['body', 'msg', 'changes'],
            'repo': ['body', 'msg', 'repository', 'full_name'],
            'repo_url_ro': ['body', 'msg', 'repository', 'html_url'],
        }
        msg = self._format_msg(filter_map, gh_msg)
        return self.formatter.fmt_pr_edited(msg)

    def pr_labeled(self, gh_msg):
        return self._pr_label_handler(gh_msg)

    def pr_unlabeled(self, gh_msg):
        return self._pr_label_handler(gh_msg)

    def pr_synchronize(self, gh_msg):
        return self._pr_handler(gh_msg)

    def status(self, msg):
        pass

    def _repo_match(self, msg):
        if self.repo_name is None:
            return True

        msg_repo = get_from_dict(msg,
                                 ['body', 'msg', 'repository', 'full_name'])
        if msg_repo == self.repo_name:
            return True

        return False

    def consume(self, msg):
        if not self._repo_match(msg):
            # Not our repo
            return

        msg_pretty = StringIO.StringIO()
        pprint(msg, msg_pretty)
        self.log.debug(msg_pretty.getvalue())
        msg_pretty.close()

        method = self.topic_mapping.get(msg.get('topic'))
        if method:
            try:
                method(msg)
            except Exception as e:
                self.log.exception("Failed with: %s", e)


class BindDyndbLDAPGithubConsumer(GithubConsumer):
    repo_name = 'freeipa/bind-dyndb-ldap'
    project = 'bind-dyndb-ldap'
    config_key = 'binddyndbldapgithubconsumer'


class SSSDGithubConsumer(GithubConsumer):
    repo_name = 'SSSD/sssd'
    project = 'sssd'
    config_key = 'sssdgithubconsumer'


class FreeIPAGithubConsumer(GithubConsumer):
    repo_name = 'freeipa/freeipa'
    project = 'freeipa'
    config_key = 'freeipagithubconsumer'


class TestGithubConsumer(GithubConsumer):
    repo_name = 'bastiak/ipa-devel-tools'
    project = 'test'
    config_key = 'testgithubconsumer'
