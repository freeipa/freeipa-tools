#
# Copyright (C) 2015
#

import os
import re
import subprocess

COMMIT_RE = r"^commit (\w+)$"
AUTHOR_RE = r"^Author: (.+) <(.+)>$"
DATE_RE = r"^Date:[ ]+(.+)$"
DESCRIPTION_RE = r"^    (.*)$"
REVIEWER_RE = r"^\s*Reviewed-By: (.+) <(.+)>$"
TICKET_RE = r"^\s*[\w: ]*\s*https://fedorahosted.org/freeipa/ticket/(\d+)\s*$"
TICKET_RE2 = r"^\s*[\w: ]*\s*https://pagure.io/freeipa/issue/(\d+)\s*$"
RELEASENOTE_RE = r"^\s*RN:\s+(.*)$"

class GitCommit(object):
    def __init__(self, commit='', author='', date='',
                 summary='', description='', reviewers=None, release_note=None):
        self.commit = commit
        self.author = author
        self.date = date
        self.summary = summary
        self.description = description
        self.reviewers = reviewers or []
        self.tickets = set()
        self.release_note = release_note or []

    def __str__(self):
        return '\n'.join([
            'Commit: %s' % self.commit,
            'Author: %s <%s>' % (self.author.name, self.author.mail),
            'Date: %s' % self.date,
            'Summary: %s' % self.summary,
            'Tickets: %s' % self._tickets(),
            'Release notes: %s' % ' '.join(self.release_note)
        ])

    def to_short(self):
        return "%s %s %s\t %s" % (
            self.commit, self.date, self.summary, self._tickets())

    def _tickets(self):
        tickets = ['#' + t for t in self.tickets]
        return ', '.join(sorted(list(tickets)))


class GitAuthor(object):
    def __init__(self, name, mail):
        self.name = name
        self.mail = mail
        self.commits = []
        self.reviews = []
        self.tickets = set()

    def __str__(self):
        return (u"%s <%s> %s %s %s" % (
            self.name, self.mail, len(self.commits), len(self.reviews),
            len(self.tickets)))

    def __unicode__(self):
        return self.__str__()

    def stats(self):
        return u'\n'.join([
            u'Commits: %s' % len(self.commits),
            u'Reviews: %s' % len(self.reviews),
            u'Tickets: %s' % len(self.tickets)
        ])


class GitInfo():
    def __init__(self, repopath):
        self.repopath = repopath
        self.commits = []
        self.authors = dict()

    def __str__(self):
        out = [
            "Commits: %s" % len(self.commits),
        ]
        for commit in self.commits:
            out.append(str(commit))
            out.append('')
        return '\n'.join(out)

    def load(self, since):
        datestr = since.isoformat()
        cmd = ["git", "log", "--use-mailmap", "--since", datestr]
        os.chdir(self.repopath)
        result = subprocess.check_output(cmd, stderr=subprocess.STDOUT, encoding='utf-8')
        self.add_commits(result)

    def get_log(self, revision_range):
        cmd = ["git", "log", "--use-mailmap", revision_range]
        os.chdir(self.repopath)
        result = subprocess.check_output(cmd, stderr=subprocess.STDOUT, encoding='utf-8')
        return result

    def add_commits(self, git_output):
        lines = git_output.splitlines()
        commit = None
        description = None
        del self.commits[:]
        self.authors.clear()
        for line in lines:
            commit_hash = re.match(COMMIT_RE, line)
            if commit_hash:
                commit = GitCommit()
                self.commits.append(commit)
                description = []
                commit.commit = commit_hash.groups()[0]

            author_g = re.match(AUTHOR_RE, line)
            if author_g:
                name = author_g.groups()[0]
                mail = author_g.groups()[1]
                author = self.get_add_author(name, mail)
                commit.author = author
                author.commits.append(commit)

            date = re.match(DATE_RE, line)
            if date:
                commit.date = date.groups()[0]

            description_line = re.match(DESCRIPTION_RE, line)
            if description_line:
                description.append(description_line.groups()[0])
                commit.summary = description[0]
                commit.description = '\n'.join(description)

        for commit in self.commits:
            self.parse_description(commit)

    def get_add_author(self, name, mail):
        author = self.authors.get(mail, None)
        if not author:
            author = GitAuthor(name, mail)
            self.authors[mail] = author
        return author

    def _get_ticket(self, line, commit):
        for regex in [TICKET_RE, TICKET_RE2]:
            ticket_g = re.match(regex, line)
            if ticket_g:
                ticket_id = ticket_g.groups()[0]
                commit.author.tickets.add(ticket_id)
                commit.tickets.add(ticket_id)

    def parse_description(self, commit):

        lines = commit.description.splitlines()
        for line in lines:
            #print line
            reviewer_g = re.match(REVIEWER_RE, line)
            if reviewer_g:
                name = reviewer_g.groups()[0]
                mail = reviewer_g.groups()[1]
                author = self.get_add_author(name, mail)
                commit.reviewers.append(author)
                author.reviews.append(commit)
            release_note = re.match(RELEASENOTE_RE, line)
            if release_note:
                commit.release_note.append(release_note.groups()[0])
            ticket_g = re.match(TICKET_RE, line)
            self._get_ticket(line, commit)

        return commit
