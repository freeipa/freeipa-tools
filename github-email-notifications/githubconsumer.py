#!/usr/bin/python2
# inspired by: https://github.com/lmacken/fedmsg-koji-consumer

import fedmsg.consumers

from pprint import pprint

def get_from_dict(data_dict, key_list):
    return reduce(lambda d, k: d[k], key_list, data_dict)

class StdoutFormatter(object):
    def fmt_issue_comment(self, comment):
        # this could be mail subject
        print "{comment_author} commented on a pull request".format(**comment)
        # this could be mail body
        print comment['comment_body']
        print "See the full comment at {comment_url}\n".format(**comment)

    def fmt_pr(self, pull_req):
        # this could be mail subject
        print "{pr_author}'s pull request #{pr_num}: \"{pr_title}\" was {pr_action}\n".format(**pull_req)
        # this could be mail body
        if pull_req['pr_action'] == u'opened':
            print "PR body:\n{pr_body}\n".format(**pull_req)
        print "See the full pull-request at {pr_url}\n".format(**pull_req)

class RawPPFormatter(object):
    def fmt_issue_comment(self, comment):
        pprint(comment)

    def fmt_pr(self, action, pull_req):
        pprint(pull_req)

class GithubConsumer(fedmsg.consumers.FedmsgConsumer):
    topic = 'org.fedoraproject.prod.github.*'
    config_key = 'githubconsumer'
    formatter_cls = RawPPFormatter

    def __init__(self, *args, **kw):
        super(GithubConsumer, self).__init__(*args, **kw)

        self.topic_mapping = {
            'org.fedoraproject.prod.github.issue.comment': self.issue_comment,
            'org.fedoraproject.prod.github.issue.labeled': self.issue_labeled,
            'org.fedoraproject.prod.github.pull_request.opened': self.pr_opened,
            'org.fedoraproject.prod.github.pull_request.reopened': self.pr_reopened,
            'org.fedoraproject.prod.github.pull_request.closed': self.pr_closed,
            'org.fedoraproject.prod.github.pull_request.review_comment': self.pr_review,
            'org.fedoraproject.prod.github.status': self.status,
        }
        self.repo_name = None
        self.formatter = self.formatter_cls()

    def _format_msg(self, filter_map, gh_msg):
        msg = dict()
        for fld, klist in filter_map.iteritems():
            msg[fld] = get_from_dict(gh_msg, klist)
        return msg

    def _pr_handler(self, gh_msg):
        filter_map = { 'pr_url' : ['body', 'msg', 'pull_request', 'html_url'],
                       'pr_author' : ['body', 'msg', 'pull_request', 'user', 'login'],
                       'pr_title' : ['body', 'msg', 'pull_request', 'title'],
                       'pr_body' : ['body', 'msg', 'pull_request', 'body'],
                       'pr_num' : ['body', 'msg', 'number'],
                       'pr_action' : ['body', 'msg', 'action'] }
        msg = self._format_msg(filter_map, gh_msg)
        return self.formatter.fmt_pr(msg)

    def issue_comment(self, gh_msg):
        pr_link = get_from_dict(gh_msg, ['body', 'msg', 'issue', 'pull_request'])
        if not pr_link:
            # We only care about comments in pull-requests
            return

        filter_map = { 'comment_url' : ['body', 'msg', 'comment', 'html_url'],
                       'comment_author' : ['body', 'msg', 'comment', 'user', 'login'],
                       'comment_body' : ['body', 'msg', 'comment', 'body'] }
        msg = self._format_msg(filter_map, gh_msg)
        return self.formatter.fmt_issue_comment(msg)

    def issue_labeled(self, msg):
        pass

    def pr_opened(self, gh_msg):
        return self._pr_handler(gh_msg)

    def pr_closed(self, gh_msg):
        return self._pr_handler(gh_msg)

    def pr_reopened(self, gh_msg):
        return self._pr_handler(gh_msg)

    def pr_review(self, msg):
        pass

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

        pprint(msg)
        method = self.topic_mapping.get(msg.get('topic'))
        if method:
            method(msg)

class TestRepoConsumer(GithubConsumer):
    config_key = 'testrepoconsumer'
    repo_name = u'jhrozek/testrepo'
    formatter_cls = StdoutFormatter

    def __init__(self, *args, **kw):
        super(TestRepoConsumer, self).__init__(*args, **kw)
