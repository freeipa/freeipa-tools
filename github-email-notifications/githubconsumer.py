#!/usr/bin/python2
# inspired by: https://github.com/lmacken/fedmsg-koji-consumer

import fedmsg.consumers

from pprint import pprint


class GithubConsumer(fedmsg.consumers.FedmsgConsumer):
    topic = 'org.fedoraproject.prod.github.*'
    config_key = 'githubconsumer'

    def __init__(self, *args, **kw):
        super(GithubConsumer, self).__init__(*args, **kw)

        self.topic_mapping = {
            'org.fedoraproject.prod.github.issue.comment': self.issue_comment,
            'org.fedoraproject.prod.github.issue.labeled': self.issue_labeled,
            'org.fedoraproject.prod.github.pull_request.opened': self.pr_opened,
            'org.fedoraproject.prod.github.pull_request.closed': self.pr_closed,
            'org.fedoraproject.prod.github.status': self.status,
        }

    def issue_comment(self, msg):
        pprint(msg)

    def issue_labeled(self, msg):
        pass

    def pr_opened(self, msg):
        pprint(msg)

    def pr_closed(self, msg):
        pprint(msg)

    def status(self, msg):
        pass

    def consume(self, msg):
        method = self.topic_mapping.get(msg.get('topic'))
        if method:
            method(msg)
